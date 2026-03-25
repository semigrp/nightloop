use std::{
    collections::{HashMap, HashSet},
    fs,
    time::Instant,
};

use anyhow::{anyhow, Result};
use chrono::Utc;

use crate::{
    agent_exec, budget,
    config::Config,
    diff_budget::{self, DiffStat},
    estimate::{self, EstimateBasis},
    git_ops,
    github::GitHubClient,
    issue_lint::{self, LintFinding},
    issue_parse,
    models::{ChildIssue, IssueEstimate, IssueSnapshot, ParentIssue, RunRecord},
    reporting, selection, telemetry,
};

#[derive(Debug)]
pub struct RunReport {
    pub ok: bool,
    pub lines: Vec<Vec<(String, String)>>,
}

impl RunReport {
    pub fn print(&self) {
        for line in &self.lines {
            let pairs = line
                .iter()
                .map(|(key, value)| (key.as_str(), value.clone()))
                .collect::<Vec<_>>();
            reporting::print_pairs(&pairs);
        }
    }
}

#[derive(Debug)]
struct PreparedIssue {
    snapshot: IssueSnapshot,
    child: Option<ChildIssue>,
    estimate: Option<IssueEstimate>,
    reasons: Vec<String>,
    lint_findings: Vec<LintFinding>,
    status: String,
    actual_minutes: Option<u32>,
    branch: Option<String>,
    pr_url: Option<String>,
    copilot_review: Option<String>,
}

struct PreparedCampaign {
    parent: ParentIssue,
    issues: Vec<PreparedIssue>,
    issue_snapshots: HashMap<u64, IssueSnapshot>,
    dependency_done_cache: HashMap<u64, bool>,
    estimated_total_minutes: u32,
    remaining_minutes: u32,
    hours: u32,
    target_repo_match: String,
    target_repo_root: String,
    run_root: String,
}

pub fn dry_run(config: &Config, parent_issue_number: u64, hours: u32) -> Result<RunReport> {
    let github = GitHubClient::new(config);
    github.check_auth()?;
    let target_repo_match = preflight_target_repo(config)?;
    let prepared = prepare_campaign(
        config,
        &github,
        parent_issue_number,
        hours,
        target_repo_match,
    )?;
    Ok(build_report(&prepared, true, true, 0))
}

pub fn run_campaign(config: &Config, parent_issue_number: u64, hours: u32) -> Result<RunReport> {
    let github = GitHubClient::new(config);
    github.check_auth()?;
    let target_repo_match = preflight_target_repo(config)?;
    let mut prepared = prepare_campaign(
        config,
        &github,
        parent_issue_number,
        hours,
        target_repo_match,
    )?;
    let workdir = config.working_directory();
    git_ops::ensure_clean_worktree(&workdir)?;
    git_ops::switch_branch(&workdir, &config.github.base_branch)?;

    let run_id = format!(
        "{}-parent-{}",
        Utc::now().format("%Y%m%dT%H%M%SZ"),
        parent_issue_number
    );
    let run_root = config.run_root().join(&run_id);
    fs::create_dir_all(&run_root)?;

    let mut completed_in_run = HashSet::new();
    let mut previous_success_branch: Option<String> = None;
    let mut actual_total_minutes = 0u32;
    let mut ok = true;

    for item in &mut prepared.issues {
        if item.status != "selected" {
            continue;
        }

        let child = item
            .child
            .as_ref()
            .ok_or_else(|| anyhow!("selected issue missing parsed child"))?;
        if !dependencies_satisfied_live(
            child,
            &prepared.issue_snapshots,
            &mut prepared.dependency_done_cache,
            &github,
            config,
            &completed_in_run,
        )? {
            item.status = "skipped".to_string();
            item.reasons = vec!["dependency_unsatisfied".to_string()];
            ok = false;
            continue;
        }

        let estimate = item
            .estimate
            .as_ref()
            .ok_or_else(|| anyhow!("selected issue missing estimate"))?;
        let child_run_dir = run_root.join(format!("child-{}", child.number));
        fs::create_dir_all(&child_run_dir)?;
        fs::write(
            child_run_dir.join("issue.json"),
            serde_json::to_string_pretty(&item.snapshot)?,
        )?;
        fs::write(child_run_dir.join("issue.md"), &item.snapshot.body)?;

        let branch_name = format!("nightloop/{}-{}", parent_issue_number, child.number);
        let pr_base = previous_success_branch
            .clone()
            .unwrap_or_else(|| config.github.base_branch.clone());
        let base_sha = if previous_success_branch.is_some() {
            git_ops::rev_parse(&workdir, "HEAD")?
        } else {
            git_ops::rev_parse(&workdir, &config.github.base_branch)?
        };

        github.remove_labels(child.number, &[&config.labels.ready, &config.labels.review])?;
        github.add_labels(child.number, &[&config.labels.running])?;
        git_ops::create_branch(&workdir, &branch_name, &base_sha)?;

        let prompt = build_agent_prompt(&prepared.parent, child, estimate, config);
        let prompt_path = child_run_dir.join("agent-prompt.md");
        fs::write(&prompt_path, &prompt)?;
        item.branch = Some(branch_name.clone());

        let min_diff = config.diff.min_lines.max(child.target_size.min_lines());
        let max_diff = config.diff.max_lines.min(child.target_size.max_lines());
        let envs = vec![
            (
                "NIGHTLOOP_PARENT_ISSUE".to_string(),
                prepared.parent.number.to_string(),
            ),
            (
                "NIGHTLOOP_CHILD_ISSUE".to_string(),
                child.number.to_string(),
            ),
            ("NIGHTLOOP_CHILD_TITLE".to_string(), child.title.clone()),
            (
                "NIGHTLOOP_MODEL_PROFILE".to_string(),
                estimate.model_profile.clone(),
            ),
            ("NIGHTLOOP_MODEL".to_string(), estimate.model.clone()),
            (
                "NIGHTLOOP_REASONING_EFFORT".to_string(),
                estimate.reasoning_effort.clone(),
            ),
            (
                "NIGHTLOOP_PROMPT_FILE".to_string(),
                prompt_path.display().to_string(),
            ),
            (
                "NIGHTLOOP_RUN_DIR".to_string(),
                child_run_dir.display().to_string(),
            ),
            ("NIGHTLOOP_BASE_BRANCH".to_string(), pr_base.clone()),
            ("NIGHTLOOP_DIFF_MIN".to_string(), min_diff.to_string()),
            ("NIGHTLOOP_DIFF_MAX".to_string(), max_diff.to_string()),
        ];

        let started = Instant::now();
        let agent_result =
            agent_exec::run_shell_command(&config.agent.command, &workdir, &envs, None)?;
        fs::write(child_run_dir.join("agent.stdout"), &agent_result.stdout)?;
        fs::write(child_run_dir.join("agent.stderr"), &agent_result.stderr)?;

        let mut failure_code = None;
        if !agent_result.success() {
            failure_code = Some("agent_command_failed".to_string());
        }

        if failure_code.is_none() {
            for (index, command) in child.verification.iter().enumerate() {
                let result =
                    agent_exec::run_shell_command(&command.command, &workdir, &envs, None)?;
                fs::write(
                    child_run_dir.join(format!("verification-{:02}.stdout", index + 1)),
                    &result.stdout,
                )?;
                fs::write(
                    child_run_dir.join(format!("verification-{:02}.stderr", index + 1)),
                    &result.stderr,
                )?;
                if !result.success() {
                    failure_code = Some("verification_failed".to_string());
                    break;
                }
            }
        }

        let diff_stat = git_ops::diff_against(&workdir, &base_sha).unwrap_or(DiffStat {
            changed_lines: 0,
            files_touched: 0,
        });
        if failure_code.is_none() {
            if let Err(err) = diff_budget::enforce_diff_budget(config, child, diff_stat) {
                failure_code = Some(err.to_string());
            }
        }

        let actual_minutes = ((started.elapsed().as_secs() + 59) / 60) as u32;
        item.actual_minutes = Some(actual_minutes);
        actual_total_minutes += actual_minutes;

        if let Some(code) = failure_code {
            ok = false;
            finalize_failure(
                config,
                &github,
                child,
                estimate,
                prepared.parent.number,
                &run_id,
                &branch_name,
                &pr_base,
                &code,
                diff_stat,
                actual_minutes,
            )?;
            item.status = "blocked".to_string();
            item.reasons = vec![code];
            let clean = git_ops::ensure_clean_worktree(&workdir).is_ok();
            if !clean || config.loop_cfg.stop_on_failure {
                break;
            }
            if let Some(previous) = &previous_success_branch {
                git_ops::switch_branch(&workdir, previous)?;
            } else {
                git_ops::switch_branch(&workdir, &config.github.base_branch)?;
            }
            continue;
        }

        git_ops::commit_all(
            &workdir,
            &format!("nightloop: issue #{} {}", child.number, child.title),
        )?;
        let pr_url = github.create_draft_pr(
            &pr_base,
            &branch_name,
            &format!("[Task #{}] {}", child.number, child.title),
            &format!(
                "Implements #{}\n\nSource of truth:\n{}\n\nVerification:\n{}",
                child.number,
                child.source_of_truth_raw,
                child
                    .verification
                    .iter()
                    .map(|command| format!("- `{}`", command.command))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        )?;
        item.pr_url = Some(pr_url.clone());

        if config.github.request_copilot_review {
            match github.request_pr_review(&pr_url, &config.github.copilot_reviewer) {
                Ok(()) => item.copilot_review = Some("requested".to_string()),
                Err(_) => {
                    item.copilot_review = Some("failed".to_string());
                    item.reasons
                        .push("copilot_review_request_failed".to_string());
                }
            }
        }

        github.remove_labels(child.number, &[&config.labels.running])?;
        github.add_labels(child.number, &[&config.labels.review])?;
        github.comment_issue(
            child.number,
            &format!(
                "nightloop success\n\n- branch: `{}`\n- draft PR: {}\n- estimated minutes: {}\n- actual minutes: {}\n- changed lines: {}\n{}",
                branch_name,
                pr_url,
                estimate.estimated_minutes,
                actual_minutes,
                diff_stat.changed_lines,
                build_copilot_review_comment_line(item.copilot_review.as_deref())
            ),
        )?;

        telemetry::append_run_record(
            &config.telemetry_history_path(),
            &RunRecord {
                run_id: run_id.clone(),
                parent_issue: prepared.parent.number,
                issue_number: child.number,
                issue_title: child.title.clone(),
                model_profile: estimate.model_profile.clone(),
                model: estimate.model.clone(),
                reasoning_effort: estimate.reasoning_effort.clone(),
                target_size: child.target_size.clone(),
                docs_impact: child.docs_impact.clone(),
                estimated_minutes: estimate.estimated_minutes,
                actual_minutes,
                changed_lines: diff_stat.changed_lines,
                files_touched: diff_stat.files_touched,
                success: true,
                status: "success".to_string(),
                copilot_review: item.copilot_review.clone(),
                branch: branch_name.clone(),
                pr_base: pr_base.clone(),
                pr_url: Some(pr_url.clone()),
                recorded_at: Utc::now(),
            },
        )?;

        item.status = "completed".to_string();
        completed_in_run.insert(child.number);
        previous_success_branch = Some(branch_name);
    }

    github.comment_issue(
        prepared.parent.number,
        &build_parent_summary(
            &prepared,
            hours,
            prepared.estimated_total_minutes,
            actual_total_minutes,
        ),
    )?;

    Ok(build_report(&prepared, false, ok, actual_total_minutes))
}

fn prepare_campaign(
    config: &Config,
    github: &GitHubClient<'_>,
    parent_issue_number: u64,
    hours: u32,
    target_repo_match: String,
) -> Result<PreparedCampaign> {
    let available_minutes = budget::available_minutes(
        hours,
        config.loop_cfg.fixed_overhead_minutes,
        config.loop_cfg.min_hours,
        config.loop_cfg.max_hours,
    )?;
    let parent_snapshot = github.view_issue(parent_issue_number)?;
    let parent = issue_parse::parse_parent_issue(&parent_snapshot)?;

    let mut issue_snapshots = HashMap::new();
    for child_ref in &parent.children {
        issue_snapshots.insert(child_ref.number, github.view_issue(child_ref.number)?);
    }

    let default_basis = EstimateBasis::from_cli_str(&config.estimation.default_basis)
        .unwrap_or(EstimateBasis::Hybrid);
    let mut dependency_done_cache = HashMap::new();
    for snapshot in issue_snapshots.values() {
        dependency_done_cache.insert(snapshot.number, issue_done_on_github(snapshot, config));
    }

    let mut planned_numbers = HashSet::new();
    let mut estimated_total_minutes = 0u32;
    let mut issues = Vec::new();

    for child_ref in &parent.children {
        let snapshot = issue_snapshots
            .get(&child_ref.number)
            .cloned()
            .ok_or_else(|| anyhow!("missing fetched child issue {}", child_ref.number))?;
        let lint = issue_lint::lint_child_issue(config, &snapshot);
        let mut prepared = PreparedIssue {
            snapshot: snapshot.clone(),
            child: lint.child,
            estimate: None,
            reasons: Vec::new(),
            lint_findings: lint.findings.clone(),
            status: "skipped".to_string(),
            actual_minutes: None,
            branch: None,
            pr_url: None,
            copilot_review: if config.github.request_copilot_review {
                Some("skipped".to_string())
            } else {
                None
            },
        };

        if !lint.valid {
            prepared.reasons = lint
                .findings
                .iter()
                .map(|finding| finding.code.clone())
                .collect();
            issues.push(prepared);
            continue;
        }

        let child = prepared
            .child
            .as_ref()
            .ok_or_else(|| anyhow!("valid lint report missing parsed child"))?;
        prepared.estimate = Some(estimate::estimate_child_issue(
            config,
            child,
            default_basis,
        )?);
        prepared.reasons = selection::static_eligibility_reasons(config, child);

        if prepared.reasons.is_empty()
            && !dependencies_satisfied_live(
                child,
                &issue_snapshots,
                &mut dependency_done_cache,
                github,
                config,
                &planned_numbers,
            )?
        {
            prepared.reasons.push("dependency_unsatisfied".to_string());
        }

        if prepared.reasons.is_empty() {
            let minutes = prepared
                .estimate
                .as_ref()
                .map(|estimate| estimate.estimated_minutes)
                .unwrap_or_default();
            if selection::pack_issue_if_fit(minutes, estimated_total_minutes, available_minutes) {
                prepared.status = "selected".to_string();
                estimated_total_minutes += minutes;
                planned_numbers.insert(child.number);
            } else {
                prepared.reasons.push("budget_exhausted".to_string());
            }
        }

        issues.push(prepared);
    }

    let remaining_minutes = available_minutes.saturating_sub(estimated_total_minutes);
    Ok(PreparedCampaign {
        parent,
        issues,
        issue_snapshots,
        dependency_done_cache,
        estimated_total_minutes,
        remaining_minutes,
        hours,
        target_repo_match,
        target_repo_root: config.target_repo_root().display().to_string(),
        run_root: config.run_root().display().to_string(),
    })
}

fn dependencies_satisfied_live(
    child: &ChildIssue,
    issue_snapshots: &HashMap<u64, IssueSnapshot>,
    dependency_done_cache: &mut HashMap<u64, bool>,
    github: &GitHubClient<'_>,
    config: &Config,
    planned_or_completed: &HashSet<u64>,
) -> Result<bool> {
    let done_from_known = dependency_numbers_done(issue_snapshots, config);
    if selection::dependencies_satisfied(child, &done_from_known, planned_or_completed) {
        return Ok(true);
    }

    for dependency in &child.dependencies {
        if planned_or_completed.contains(dependency) {
            continue;
        }
        if let Some(done) = dependency_done_cache.get(dependency) {
            if *done {
                continue;
            }
            return Ok(false);
        }
        let snapshot = github.view_issue(*dependency)?;
        let done = issue_done_on_github(&snapshot, config);
        dependency_done_cache.insert(*dependency, done);
        if !done {
            return Ok(false);
        }
    }
    Ok(true)
}

fn dependency_numbers_done(
    issue_snapshots: &HashMap<u64, IssueSnapshot>,
    config: &Config,
) -> HashSet<u64> {
    issue_snapshots
        .values()
        .filter(|snapshot| issue_done_on_github(snapshot, config))
        .map(|snapshot| snapshot.number)
        .collect()
}

fn issue_done_on_github(snapshot: &IssueSnapshot, config: &Config) -> bool {
    snapshot.state.as_str() == "closed" || snapshot.has_label(&config.labels.done)
}

fn finalize_failure(
    config: &Config,
    github: &GitHubClient<'_>,
    child: &ChildIssue,
    estimate: &IssueEstimate,
    parent_issue: u64,
    run_id: &str,
    branch_name: &str,
    pr_base: &str,
    failure_code: &str,
    diff_stat: DiffStat,
    actual_minutes: u32,
) -> Result<()> {
    github.remove_labels(child.number, &[&config.labels.running])?;
    github.add_labels(child.number, &[&config.labels.blocked])?;
    github.comment_issue(
        child.number,
        &format!(
            "nightloop failure\n\n- reason: `{}`\n- branch: `{}`\n- actual minutes: {}\n- changed lines: {}",
            failure_code, branch_name, actual_minutes, diff_stat.changed_lines
        ),
    )?;

    telemetry::append_run_record(
        &config.telemetry_history_path(),
        &RunRecord {
            run_id: run_id.to_string(),
            parent_issue,
            issue_number: child.number,
            issue_title: child.title.clone(),
            model_profile: estimate.model_profile.clone(),
            model: estimate.model.clone(),
            reasoning_effort: estimate.reasoning_effort.clone(),
            target_size: child.target_size.clone(),
            docs_impact: child.docs_impact.clone(),
            estimated_minutes: estimate.estimated_minutes,
            actual_minutes,
            changed_lines: diff_stat.changed_lines,
            files_touched: diff_stat.files_touched,
            success: false,
            status: failure_code.to_string(),
            copilot_review: None,
            branch: branch_name.to_string(),
            pr_base: pr_base.to_string(),
            pr_url: None,
            recorded_at: Utc::now(),
        },
    )?;
    Ok(())
}

fn build_agent_prompt(
    parent: &ParentIssue,
    child: &ChildIssue,
    estimate: &IssueEstimate,
    config: &Config,
) -> String {
    let verification = child
        .verification
        .iter()
        .map(|command| format!("- {}", command.command))
        .collect::<Vec<_>>()
        .join("\n");
    let docs_note = if child.docs_impact.as_str() == "none" {
        "Documentation impact: none".to_string()
    } else {
        format!(
            "Documentation impact: {}. Update the required docs in the same branch.",
            child.docs_impact.as_str()
        )
    };

    format!(
        "Implement GitHub child issue #{} from parent campaign #{}.\n\n\
Read AGENTS.md and the listed source-of-truth refs before changing code.\n\
Keep the diff reviewable and within the declared target change size.\n\
Do not widen scope.\n\
\n\
Parent issue: #{} {}\n\
Child issue: #{} {}\n\
Suggested model profile: {}\n\
Exact model: {}\n\
Reasoning effort: {}\n\
Target change size: {}\n\
{}\n\
\n\
Background:\n{}\n\
\n\
Goal:\n{}\n\
\n\
Scope:\n{}\n\
\n\
Out of scope:\n{}\n\
\n\
Source of truth:\n{}\n\
\n\
Acceptance criteria:\n{}\n\
\n\
Verification:\n{}\n\
\n\
Implementation constraints:\n{}\n\
\n\
Repository guidance:\n- Working directory: {}\n- Base branch: {}\n",
        child.number,
        parent.number,
        parent.number,
        parent.title,
        child.number,
        child.title,
        estimate.model_profile,
        estimate.model,
        estimate.reasoning_effort,
        child.target_size.as_str(),
        docs_note,
        child.background,
        child.goal,
        child.scope,
        child.out_of_scope,
        child.source_of_truth_raw,
        child.acceptance_criteria,
        verification,
        child
            .implementation_constraints
            .clone()
            .unwrap_or_else(|| "none".to_string()),
        config.working_directory().display(),
        config.github.base_branch
    )
}

fn build_parent_summary(
    prepared: &PreparedCampaign,
    hours: u32,
    estimated_total_minutes: u32,
    actual_total_minutes: u32,
) -> String {
    let selected = prepared
        .issues
        .iter()
        .filter(|item| {
            item.status == "selected" || item.status == "completed" || item.status == "blocked"
        })
        .map(|item| format!("#{}", item.snapshot.number))
        .collect::<Vec<_>>();
    let completed = prepared
        .issues
        .iter()
        .filter(|item| item.status == "completed")
        .map(|item| match &item.pr_url {
            Some(pr_url) => format!("#{} ({})", item.snapshot.number, pr_url),
            None => format!("#{}", item.snapshot.number),
        })
        .collect::<Vec<_>>();
    let blocked = prepared
        .issues
        .iter()
        .filter(|item| item.status == "blocked")
        .map(|item| format!("#{} ({})", item.snapshot.number, item.reasons.join(",")))
        .collect::<Vec<_>>();
    let skipped = prepared
        .issues
        .iter()
        .filter(|item| item.status == "skipped")
        .map(|item| format!("#{} ({})", item.snapshot.number, item.reasons.join(",")))
        .collect::<Vec<_>>();
    let pr_chain = prepared
        .issues
        .iter()
        .filter_map(|item| {
            item.pr_url
                .as_ref()
                .map(|url| format!("#{} -> {}", item.snapshot.number, url))
        })
        .collect::<Vec<_>>();
    let copilot_reviews = prepared
        .issues
        .iter()
        .filter_map(|item| {
            item.copilot_review
                .as_ref()
                .and_then(|status| match status.as_str() {
                    "requested" | "failed" => {
                        Some(format!("#{} ({})", item.snapshot.number, status))
                    }
                    _ => None,
                })
        })
        .collect::<Vec<_>>();
    let mut summary = format!(
        "nightloop parent summary\n\n- Hours: {}\n- Selected children: {}\n- Completed children: {}\n- Blocked children: {}\n- Skipped children: {}\n- PR chain: {}",
        hours,
        join_for_comment(&selected),
        join_for_comment(&completed),
        join_for_comment(&blocked),
        join_for_comment(&skipped),
        join_for_comment(&pr_chain),
    );
    if prepared
        .issues
        .iter()
        .any(|item| item.copilot_review.is_some())
    {
        summary.push_str(&format!(
            "\n- Copilot reviews: {}",
            join_for_comment(&copilot_reviews)
        ));
    }
    summary.push_str(&format!(
        "\n- Estimated minutes: {}\n- Actual minutes: {}",
        estimated_total_minutes, actual_total_minutes
    ));
    summary
}

fn build_report(
    prepared: &PreparedCampaign,
    dry_run: bool,
    ok: bool,
    actual_total_minutes: u32,
) -> RunReport {
    let selected_count = prepared
        .issues
        .iter()
        .filter(|item| {
            item.status == "selected" || item.status == "completed" || item.status == "blocked"
        })
        .count();
    let completed_count = prepared
        .issues
        .iter()
        .filter(|item| item.status == "completed")
        .count();
    let blocked_count = prepared
        .issues
        .iter()
        .filter(|item| item.status == "blocked")
        .count();
    let skipped_count = prepared
        .issues
        .iter()
        .filter(|item| item.status == "skipped")
        .count();

    let mut lines = vec![vec![
        (
            "parent_issue".to_string(),
            prepared.parent.number.to_string(),
        ),
        ("hours".to_string(), prepared.hours.to_string()),
        ("dry_run".to_string(), dry_run.to_string()),
        ("selected_count".to_string(), selected_count.to_string()),
        ("completed_count".to_string(), completed_count.to_string()),
        ("blocked_count".to_string(), blocked_count.to_string()),
        ("skipped_count".to_string(), skipped_count.to_string()),
        (
            "estimated_total_minutes".to_string(),
            prepared.estimated_total_minutes.to_string(),
        ),
        (
            "remaining_minutes".to_string(),
            prepared.remaining_minutes.to_string(),
        ),
        (
            "actual_total_minutes".to_string(),
            actual_total_minutes.to_string(),
        ),
        (
            "target_repo_root".to_string(),
            prepared.target_repo_root.clone(),
        ),
        ("run_root".to_string(), prepared.run_root.clone()),
        (
            "target_repo_match".to_string(),
            prepared.target_repo_match.clone(),
        ),
    ]];

    for item in &prepared.issues {
        let mut line = vec![
            ("child_issue".to_string(), item.snapshot.number.to_string()),
            ("status".to_string(), item.status.clone()),
        ];
        if let Some(estimate) = &item.estimate {
            line.push((
                "estimated_minutes".to_string(),
                estimate.estimated_minutes.to_string(),
            ));
            line.push(("basis_used".to_string(), estimate.basis_used.clone()));
        }
        if !item.lint_findings.is_empty() && item.status == "skipped" && item.child.is_none() {
            line.push((
                "lint_findings".to_string(),
                item.lint_findings
                    .iter()
                    .map(|finding| finding.code.clone())
                    .collect::<Vec<_>>()
                    .join(","),
            ));
        }
        if let Some(actual_minutes) = item.actual_minutes {
            line.push(("actual_minutes".to_string(), actual_minutes.to_string()));
        }
        if !item.reasons.is_empty() {
            line.push(("reasons".to_string(), item.reasons.join(",")));
        }
        if let Some(branch) = &item.branch {
            line.push(("branch".to_string(), branch.clone()));
        }
        if let Some(pr_url) = &item.pr_url {
            line.push(("pr_url".to_string(), pr_url.clone()));
        }
        if let Some(copilot_review) = &item.copilot_review {
            line.push(("copilot_review".to_string(), copilot_review.clone()));
        }
        lines.push(line);
    }

    RunReport { ok, lines }
}

fn join_for_comment(values: &[String]) -> String {
    if values.is_empty() {
        "none".to_string()
    } else {
        values.join(", ")
    }
}

fn preflight_target_repo(config: &Config) -> Result<String> {
    let workdir = config.working_directory();
    if !workdir.exists() {
        anyhow::bail!("target_repo_missing");
    }
    git_ops::ensure_git_worktree(&workdir)?;
    match git_ops::origin_repo_slug(&workdir)? {
        Some(slug) if slug == config.repo_slug() => Ok("true".to_string()),
        Some(_) => anyhow::bail!("target_repo_mismatch"),
        None => Ok("unknown".to_string()),
    }
}

fn build_copilot_review_comment_line(status: Option<&str>) -> String {
    match status {
        Some("requested") => "- copilot review requested: true".to_string(),
        Some("failed") => {
            "- copilot review requested: failed (`copilot_review_request_failed`)".to_string()
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::{env, fs, process::Command};

    use crate::config::Config;

    use super::{build_copilot_review_comment_line, preflight_target_repo};

    #[test]
    fn copilot_review_comment_line_matches_status() {
        assert_eq!(
            build_copilot_review_comment_line(Some("requested")),
            "- copilot review requested: true"
        );
        assert!(build_copilot_review_comment_line(Some("failed"))
            .contains("copilot_review_request_failed"));
        assert!(build_copilot_review_comment_line(Some("skipped")).is_empty());
    }

    fn config_for_target(
        control: &std::path::Path,
        target: &std::path::Path,
        slug: &str,
    ) -> Config {
        let (owner, repo) = slug.split_once('/').unwrap();
        let config_path = control.join("nightloop.toml");
        fs::write(
            &config_path,
            format!(
                r#"[github]
owner = "{owner}"
repo = "{repo}"
base_branch = "main"

[agent]
command = "echo agent"
plan_command = "echo planner"
working_directory = "{}"
default_model = "gpt-5.4"
default_reasoning_effort = "medium"

[[agent.model_profiles]]
name = "balanced"
model = "gpt-5.4"
reasoning_effort = "medium"
intended_for = "default"
max_size = "M"
runtime_multiplier = 1.0

[loop]
default_hours = 4
min_hours = 2
max_hours = 6
fallback_cycle_minutes = 40
fixed_overhead_minutes = 20
stop_on_failure = true
one_branch_per_child = true
one_pr_per_child = true
run_root = ".nightloop/runs"

[diff]
min_lines = 50
max_lines = 1000
allow_doc_only_below_min = true

[labels]
campaign = "campaign"
night_run = "night-run"
ready = "agent:ready"
running = "agent:running"
review = "agent:review"
blocked = "agent:blocked"
done = "agent:done"

[docs]
required_paths = []

[estimation]
default_basis = "hybrid"
allow_ai_assist = true
template_minutes_xs = 35
template_minutes_s = 50
template_minutes_m = 80
template_minutes_l = 120
dependency_penalty_minutes = 5
docs_penalty_readme = 5
docs_penalty_user_facing = 10
docs_penalty_architecture = 15

[telemetry]
history_path = ".nightloop/history.jsonl"
min_samples_for_local = 1
local_weight = 0.65
template_weight = 0.35
"#,
                target.display()
            ),
        )
        .unwrap();
        Config::load(&config_path).unwrap()
    }

    fn git(dir: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .current_dir(dir)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {:?} failed", args);
    }

    #[test]
    fn preflight_target_repo_accepts_matching_remote_and_unknown_without_origin() {
        let root = env::temp_dir().join(format!("nightloop-preflight-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let control = root.join("control");
        let target_match = root.join("target-match");
        let target_unknown = root.join("target-unknown");
        fs::create_dir_all(&control).unwrap();
        fs::create_dir_all(&target_match).unwrap();
        fs::create_dir_all(&target_unknown).unwrap();

        git(&target_match, &["init"]);
        git(
            &target_match,
            &[
                "remote",
                "add",
                "origin",
                "git@github.com:semigrp/nightloop.git",
            ],
        );

        git(&target_unknown, &["init"]);

        let matching = config_for_target(&control, &target_match, "semigrp/nightloop");
        assert_eq!(preflight_target_repo(&matching).unwrap(), "true");

        let unknown = config_for_target(&control, &target_unknown, "semigrp/nightloop");
        assert_eq!(preflight_target_repo(&unknown).unwrap(), "unknown");
    }

    #[test]
    fn preflight_target_repo_rejects_mismatched_remote() {
        let root = env::temp_dir().join(format!(
            "nightloop-preflight-mismatch-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let control = root.join("control");
        let target = root.join("target");
        fs::create_dir_all(&control).unwrap();
        fs::create_dir_all(&target).unwrap();
        git(&target, &["init"]);
        git(
            &target,
            &["remote", "add", "origin", "git@github.com:other/repo.git"],
        );

        let config = config_for_target(&control, &target, "semigrp/nightloop");
        assert_eq!(
            preflight_target_repo(&config).unwrap_err().to_string(),
            "target_repo_mismatch"
        );
    }
}
