use std::{
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
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
}

struct PreparedCampaign {
    parent: ParentIssue,
    issues: Vec<PreparedIssue>,
    issue_snapshots: HashMap<u64, IssueSnapshot>,
    dependency_done_cache: HashMap<u64, bool>,
    estimated_total_minutes: u32,
    remaining_minutes: u32,
    hours: u32,
}

pub fn dry_run(config: &Config, parent_issue_number: u64, hours: u32) -> Result<RunReport> {
    let github = GitHubClient::new(config);
    github.check_auth()?;
    let prepared = prepare_campaign(config, &github, parent_issue_number, hours)?;
    Ok(build_report(&prepared, true, true, 0))
}

pub fn run_campaign(config: &Config, parent_issue_number: u64, hours: u32) -> Result<RunReport> {
    let github = GitHubClient::new(config);
    github.check_auth()?;
    let mut prepared = prepare_campaign(config, &github, parent_issue_number, hours)?;
    let workdir = config.working_directory();
    git_ops::ensure_clean_worktree(&workdir)?;
    git_ops::switch_branch(&workdir, &config.github.base_branch)?;

    let run_id = format!(
        "{}-parent-{}",
        Utc::now().format("%Y%m%dT%H%M%SZ"),
        parent_issue_number
    );
    let run_root = PathBuf::from(".nightloop").join("runs").join(&run_id);
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

        github.remove_labels(child.number, &[&config.labels.running])?;
        github.add_labels(child.number, &[&config.labels.review])?;
        github.comment_issue(
            child.number,
            &format!(
                "nightloop success\n\n- branch: `{}`\n- draft PR: {}\n- estimated minutes: {}\n- actual minutes: {}\n- changed lines: {}",
                branch_name, pr_url, estimate.estimated_minutes, actual_minutes, diff_stat.changed_lines
            ),
        )?;

        telemetry::append_run_record(
            &config.telemetry.history_path,
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
        &config.telemetry.history_path,
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

    format!(
        "nightloop parent summary\n\n- Hours: {}\n- Selected children: {}\n- Completed children: {}\n- Blocked children: {}\n- Skipped children: {}\n- PR chain: {}\n- Estimated minutes: {}\n- Actual minutes: {}",
        hours,
        join_for_comment(&selected),
        join_for_comment(&completed),
        join_for_comment(&blocked),
        join_for_comment(&skipped),
        join_for_comment(&pr_chain),
        estimated_total_minutes,
        actual_total_minutes
    )
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
