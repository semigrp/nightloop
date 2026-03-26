use std::{collections::HashSet, fs, path::Path, time::Instant};

use anyhow::{anyhow, bail, Result};
use chrono::Utc;

use crate::{
    agent_exec::{self, CommandRunOptions},
    budget,
    config::Config,
    diff_budget::DiffStat,
    estimate::{self, EstimateBasis},
    git_ops,
    github::GitHubClient,
    issue_lint, issue_parse,
    models::{ChildIssue, IssueEstimate, IssueSnapshot, ParentIssue, RunRecord},
    reporting, selection, telemetry,
};

#[derive(Debug)]
pub struct RunReport {
    pub ok: bool,
    pub lines: Vec<Vec<(String, String)>>,
    pub progress_lines: Vec<String>,
}

impl RunReport {
    pub fn print(&self) {
        for line in &self.progress_lines {
            reporting::print_progress(line);
        }
        for line in &self.lines {
            let pairs = line
                .iter()
                .map(|(key, value)| (key.as_str(), value.clone()))
                .collect::<Vec<_>>();
            reporting::print_pairs(&pairs);
        }
    }
}

#[derive(Debug, Clone)]
struct PreparedIssue {
    snapshot: IssueSnapshot,
    child: Option<ChildIssue>,
    estimate: Option<IssueEstimate>,
    status: String,
    reasons: Vec<String>,
    detail: Option<String>,
    branch: Option<String>,
    pr_base: Option<String>,
    pr_url: Option<String>,
    actual_minutes: Option<u32>,
    changed_lines: Option<u32>,
    files_touched: Option<u32>,
}

#[derive(Debug, Clone)]
struct PreparedCampaign {
    parent: ParentIssue,
    issues: Vec<PreparedIssue>,
    target_repo_match: String,
    estimated_total_minutes: u32,
    available_minutes: u32,
    run_root: String,
}

#[derive(Debug, Clone)]
struct Publication {
    branch_name: String,
    pr_base: String,
    push_before_pr: bool,
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
        false,
        target_repo_match,
    )?;
    Ok(build_report(&prepared, true))
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
        false,
        target_repo_match,
    )?;
    run_selected_issues(config, &github, &mut prepared, false)?;
    Ok(build_report(&prepared, false))
}

pub fn start_dry_run(config: &Config, parent_issue_number: u64) -> Result<RunReport> {
    let github = GitHubClient::new(config);
    github.check_auth()?;
    let target_repo_match = preflight_target_repo(config)?;
    let prepared = prepare_campaign(
        config,
        &github,
        parent_issue_number,
        config.loop_cfg.default_hours,
        true,
        target_repo_match,
    )?;
    Ok(build_report(&prepared, true))
}

pub fn start(config: &Config, parent_issue_number: u64) -> Result<RunReport> {
    let github = GitHubClient::new(config);
    github.check_auth()?;
    let target_repo_match = preflight_target_repo(config)?;
    let mut prepared = prepare_campaign(
        config,
        &github,
        parent_issue_number,
        config.loop_cfg.default_hours,
        true,
        target_repo_match,
    )?;
    run_selected_issues(config, &github, &mut prepared, true)?;
    Ok(build_report(&prepared, false))
}

fn prepare_campaign(
    config: &Config,
    github: &GitHubClient<'_>,
    parent_issue_number: u64,
    hours: u32,
    single_child: bool,
    target_repo_match: String,
) -> Result<PreparedCampaign> {
    let parent_snapshot = github.view_issue(parent_issue_number)?;
    let parent = issue_parse::parse_parent_issue(&parent_snapshot)?;
    let available_minutes = budget::available_minutes(
        hours,
        config.loop_cfg.fixed_overhead_minutes,
        config.loop_cfg.min_hours,
        config.loop_cfg.max_hours,
    )?;

    let mut issues = Vec::new();
    let mut estimated_total_minutes = 0;
    let mut used_minutes = 0;
    let mut done_on_github = HashSet::new();
    let mut planned = HashSet::new();

    for child_ref in &parent.children {
        let snapshot = github.view_issue(child_ref.number)?;
        if snapshot.state.as_str() == "closed" || snapshot.has_label(&config.labels.done) {
            done_on_github.insert(snapshot.number);
        }
        let lint = issue_lint::lint_child_issue(config, &snapshot);
        let mut prepared = PreparedIssue {
            snapshot: snapshot.clone(),
            child: None,
            estimate: None,
            status: "skipped".to_string(),
            reasons: Vec::new(),
            detail: None,
            branch: None,
            pr_base: None,
            pr_url: None,
            actual_minutes: None,
            changed_lines: None,
            files_touched: None,
        };

        if !lint.valid {
            prepared.reasons.push("lint_failed".to_string());
            prepared.detail = Some(
                lint.findings
                    .iter()
                    .map(|finding| finding.code.clone())
                    .collect::<Vec<_>>()
                    .join(","),
            );
            issues.push(prepared);
            continue;
        }

        let child = lint
            .child
            .ok_or_else(|| anyhow!("missing parsed child after lint success"))?;
        let basis = EstimateBasis::from_cli_str(&config.estimation.default_basis)
            .unwrap_or(EstimateBasis::Hybrid);
        let estimate = estimate::estimate_child_issue(config, &child, basis)?;
        let mut reasons = selection::static_eligibility_reasons(config, &child);
        if !selection::dependencies_satisfied(&child, &done_on_github, &planned) {
            reasons.push("dependency_unsatisfied".to_string());
        }
        if !single_child
            && !selection::pack_issue_if_fit(
                estimate.estimated_minutes,
                used_minutes,
                available_minutes,
            )
        {
            reasons.push("budget_exhausted".to_string());
        }

        if reasons.is_empty() {
            prepared.status = "selected".to_string();
            planned.insert(child.number);
            used_minutes += estimate.estimated_minutes;
            estimated_total_minutes += estimate.estimated_minutes;
            if single_child {
                prepared.child = Some(child);
                prepared.estimate = Some(estimate);
                issues.push(prepared);
                break;
            }
        } else {
            prepared.reasons = reasons;
        }

        prepared.child = Some(child);
        prepared.estimate = Some(estimate);
        issues.push(prepared);
    }

    Ok(PreparedCampaign {
        parent,
        issues,
        target_repo_match,
        estimated_total_minutes,
        available_minutes,
        run_root: config.run_root().display().to_string(),
    })
}

fn run_selected_issues(
    config: &Config,
    github: &GitHubClient<'_>,
    prepared: &mut PreparedCampaign,
    single_child: bool,
) -> Result<()> {
    ensure_minimal_safe_repairs(config, github)?;
    ensure_clean_preflight(config)?;
    let workdir = config.working_directory();
    git_ops::switch_branch(&workdir, &config.github.base_branch)?;

    let run_id = format!(
        "{}-parent-{}",
        Utc::now().format("%Y%m%dT%H%M%SZ"),
        prepared.parent.number
    );
    let run_root = config.run_root().join(&run_id);
    fs::create_dir_all(&run_root)?;

    let mut previous_success_branch: Option<String> = None;
    let mut stop = false;
    for item in &mut prepared.issues {
        if item.status != "selected" || stop {
            continue;
        }
        let child = item
            .child
            .clone()
            .ok_or_else(|| anyhow!("selected child missing"))?;
        let estimate = item
            .estimate
            .clone()
            .ok_or_else(|| anyhow!("selected estimate missing"))?;
        let child_run_dir = run_root.join(format!("child-{}", child.number));
        fs::create_dir_all(&child_run_dir)?;
        fs::write(
            child_run_dir.join("issue.json"),
            serde_json::to_string_pretty(&item.snapshot)?,
        )?;
        fs::write(child_run_dir.join("issue.md"), &item.snapshot.body)?;

        let publication = if single_child {
            Publication {
                branch_name: format!("nightloop/{}-{}", prepared.parent.number, child.number),
                pr_base: config.github.base_branch.clone(),
                push_before_pr: true,
            }
        } else {
            nightly_branch_publication(
                prepared.parent.number,
                child.number,
                &config.github.base_branch,
                previous_success_branch.as_deref(),
            )
        };
        item.branch = Some(publication.branch_name.clone());
        item.pr_base = Some(publication.pr_base.clone());

        let base_sha = if let Some(previous) = &previous_success_branch {
            if publication.pr_base == *previous {
                git_ops::rev_parse(&workdir, "HEAD")?
            } else {
                git_ops::rev_parse(&workdir, &config.github.base_branch)?
            }
        } else {
            git_ops::rev_parse(&workdir, &config.github.base_branch)?
        };

        let started_at = Instant::now();
        let outcome = (|| -> Result<(DiffStat, u32, u32, String)> {
            prepare_branch(&workdir, &publication.branch_name, &publication.pr_base)?;
            github.remove_labels(
                child.number,
                &[
                    &config.labels.ready,
                    &config.labels.blocked,
                    &config.labels.review,
                ],
            )?;
            github.add_labels(child.number, &[&config.labels.running])?;
            if single_child {
                run_plan_phase(config, &child, &estimate, &prepared.parent, &child_run_dir)?;
            }
            run_implementation_phase(
                config,
                &child,
                &estimate,
                &prepared.parent,
                &child_run_dir,
                single_child,
            )?;
            run_verification(&workdir, &child, &child_run_dir)?;

            let diff_stat = git_ops::diff_against(&workdir, &base_sha, &[config.run_root()])?;
            enforce_diff_budget(&child, &diff_stat, config)?;
            let files_touched = diff_stat.files_touched;
            let actual_minutes = started_at.elapsed().as_secs().div_ceil(60) as u32;
            let commit_message = format!("child #{}: {}", child.number, child.title);
            git_ops::commit_all(&workdir, &commit_message)?;
            if publication.push_before_pr {
                git_ops::push_current_branch(&workdir, &publication.branch_name)?;
            }
            let pr_title = format!("[{}] {}", child.number, child.title);
            let pr_body = build_pr_body(&prepared.parent, &child, &estimate, single_child);
            let pr_url = github.create_draft_pr(
                &publication.pr_base,
                &publication.branch_name,
                &pr_title,
                &pr_body,
            )?;
            Ok((diff_stat, actual_minutes, files_touched, pr_url))
        })();

        match outcome {
            Ok((diff_stat, actual_minutes, files_touched, pr_url)) => {
                let changed_lines = diff_stat.changed_lines;
                github.remove_labels(child.number, &[&config.labels.running])?;
                github.add_labels(child.number, &[&config.labels.review])?;

                item.status = "success".to_string();
                item.pr_url = Some(pr_url.clone());
                item.actual_minutes = Some(actual_minutes);
                item.changed_lines = Some(changed_lines);
                item.files_touched = Some(files_touched);
                previous_success_branch = Some(publication.branch_name.clone());

                let record = RunRecord {
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
                    changed_lines,
                    files_touched,
                    success: true,
                    status: "success".to_string(),
                    workflow: if single_child {
                        "start".to_string()
                    } else {
                        "nightly".to_string()
                    },
                    planner_used: single_child,
                    copilot_review: None,
                    review_comments_total: 0,
                    review_comments_applied: 0,
                    review_comments_ignored: 0,
                    fix_rounds: 0,
                    split_mode: None,
                    stage_index: None,
                    stage_total: None,
                    stage_completed: false,
                    active_pr_url: Some(pr_url),
                    branch: publication.branch_name.clone(),
                    pr_base: publication.pr_base.clone(),
                    pr_url: item.pr_url.clone(),
                    recorded_at: Utc::now(),
                };
                telemetry::append_run_record(&config.telemetry_history_path(), &record)?;
            }
            Err(err) => {
                let _ = github.remove_labels(child.number, &[&config.labels.running]);
                let _ = github.add_labels(child.number, &[&config.labels.blocked]);
                item.status = if err.to_string() == "split_required" {
                    "split_required".to_string()
                } else {
                    "blocked".to_string()
                };
                item.reasons = vec![err.to_string()];
                item.detail = Some(err.to_string());
                stop = config.loop_cfg.stop_on_failure || single_child;
            }
        }
    }

    github.comment_issue(
        prepared.parent.number,
        &build_parent_summary(prepared, single_child),
    )?;

    Ok(())
}

fn run_plan_phase(
    config: &Config,
    child: &ChildIssue,
    estimate: &IssueEstimate,
    parent: &ParentIssue,
    child_run_dir: &Path,
) -> Result<()> {
    let template_path = config.resolve_control_path(Path::new("prompts/plan_child_issue.md"));
    let template = fs::read_to_string(&template_path)
        .map_err(|_| anyhow!("failed to read {}", template_path.display()))?;
    let prompt = format!(
        "{template}\n\nParent: #{} {}\nChild: #{} {}\nSuggested profile: {}\nEstimated minutes: {}\n\nIssue body:\n{}\n",
        parent.number,
        parent.title,
        child.number,
        child.title,
        estimate.model_profile,
        estimate.estimated_minutes,
        child.body
    );
    let result = agent_exec::run_shell_command(
        &config.agent.plan_command,
        &config.working_directory(),
        &[(
            "NIGHTLOOP_PROMPT_FILE".to_string(),
            child_run_dir.join("plan-prompt.md").display().to_string(),
        )],
        CommandRunOptions::streaming("agent").with_stdin(&prompt),
    )?;
    fs::write(child_run_dir.join("plan-prompt.md"), &prompt)?;
    fs::write(child_run_dir.join("plan.stdout"), &result.stdout)?;
    fs::write(child_run_dir.join("plan.stderr"), &result.stderr)?;
    if !result.success() {
        bail!("plan_command_failed");
    }
    Ok(())
}

fn run_implementation_phase(
    config: &Config,
    child: &ChildIssue,
    estimate: &IssueEstimate,
    parent: &ParentIssue,
    child_run_dir: &Path,
    single_child: bool,
) -> Result<()> {
    let prompt = build_agent_prompt(parent, child, estimate, single_child);
    let result = agent_exec::run_shell_command(
        &config.agent.command,
        &config.working_directory(),
        &[(
            "NIGHTLOOP_PROMPT_FILE".to_string(),
            child_run_dir.join("agent-prompt.md").display().to_string(),
        )],
        CommandRunOptions::streaming("agent").with_stdin(&prompt),
    )?;
    fs::write(child_run_dir.join("agent-prompt.md"), &prompt)?;
    fs::write(child_run_dir.join("agent.stdout"), &result.stdout)?;
    fs::write(child_run_dir.join("agent.stderr"), &result.stderr)?;
    if !result.success() {
        bail!("agent_command_failed");
    }
    Ok(())
}

fn run_verification(workdir: &Path, child: &ChildIssue, child_run_dir: &Path) -> Result<()> {
    let mut transcripts = Vec::new();
    for (index, command) in child.verification.iter().enumerate() {
        let result = agent_exec::run_shell_command(
            &command.command,
            workdir,
            &[],
            CommandRunOptions::streaming("verify"),
        )?;
        transcripts.push(format!(
            "$ {}\n[status={}]\n{}\n{}",
            command.command, result.status_code, result.stdout, result.stderr
        ));
        fs::write(
            child_run_dir.join(format!("verify-{}.log", index + 1)),
            transcripts.last().cloned().unwrap_or_default(),
        )?;
        if !result.success() {
            bail!("verification_failed");
        }
    }
    Ok(())
}

fn enforce_diff_budget(child: &ChildIssue, diff_stat: &DiffStat, config: &Config) -> Result<()> {
    let changed_lines = diff_stat.changed_lines;
    if changed_lines > child.target_size.max_lines() || changed_lines > config.diff.max_lines {
        bail!("split_required");
    }
    if changed_lines < config.diff.min_lines
        && !(config.diff.allow_doc_only_below_min && child.allows_small_diff_exception())
    {
        bail!("diff_below_min");
    }
    Ok(())
}

fn build_agent_prompt(
    parent: &ParentIssue,
    child: &ChildIssue,
    estimate: &IssueEstimate,
    single_child: bool,
) -> String {
    format!(
        "Parent issue: #{} {}\nChild issue: #{} {}\nWorkflow: {}\nModel profile: {}\nEstimated minutes: {}\n\nImplement only the child issue scope. Run the declared verification commands before finishing.\n\nIssue body:\n{}\n",
        parent.number,
        parent.title,
        child.number,
        child.title,
        if single_child { "start" } else { "nightly" },
        estimate.model_profile,
        estimate.estimated_minutes,
        child.body
    )
}

fn build_pr_body(
    parent: &ParentIssue,
    child: &ChildIssue,
    estimate: &IssueEstimate,
    single_child: bool,
) -> String {
    format!(
        "## Summary\n- Parent issue: #{}\n- Child issue: #{}\n- Workflow: {}\n- Model profile: {}\n- Estimated minutes: {}\n",
        parent.number,
        child.number,
        if single_child { "start" } else { "nightly" },
        estimate.model_profile,
        estimate.estimated_minutes
    )
}

fn build_parent_summary(prepared: &PreparedCampaign, single_child: bool) -> String {
    let succeeded = prepared
        .issues
        .iter()
        .filter(|item| item.status == "success")
        .map(|item| format!("#{}", item.snapshot.number))
        .collect::<Vec<_>>();
    let failed = prepared
        .issues
        .iter()
        .filter(|item| {
            item.status != "success" && item.status != "selected" && !item.reasons.is_empty()
        })
        .map(|item| format!("#{} ({})", item.snapshot.number, item.reasons.join(",")))
        .collect::<Vec<_>>();

    format!(
        "nightloop {} summary\n\n- succeeded: {}\n- deferred: {}\n",
        if single_child { "start" } else { "nightly" },
        if succeeded.is_empty() {
            "none".to_string()
        } else {
            succeeded.join(", ")
        },
        if failed.is_empty() {
            "none".to_string()
        } else {
            failed.join(", ")
        }
    )
}

fn build_report(prepared: &PreparedCampaign, dry_run: bool) -> RunReport {
    let mut lines = vec![vec![
        (
            "ok".to_string(),
            prepared
                .issues
                .iter()
                .all(|item| !matches!(item.status.as_str(), "blocked" | "split_required"))
                .to_string(),
        ),
        (
            "parent_issue".to_string(),
            prepared.parent.number.to_string(),
        ),
        (
            "target_repo_match".to_string(),
            prepared.target_repo_match.clone(),
        ),
        (
            "available_minutes".to_string(),
            prepared.available_minutes.to_string(),
        ),
        (
            "estimated_total_minutes".to_string(),
            prepared.estimated_total_minutes.to_string(),
        ),
        ("run_root".to_string(), prepared.run_root.clone()),
        ("dry_run".to_string(), dry_run.to_string()),
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
        }
        if !item.reasons.is_empty() {
            line.push(("reasons".to_string(), item.reasons.join(",")));
        }
        if let Some(detail) = &item.detail {
            line.push(("detail".to_string(), detail.clone()));
        }
        if let Some(branch) = &item.branch {
            line.push(("branch".to_string(), branch.clone()));
        }
        if let Some(pr_base) = &item.pr_base {
            line.push(("pr_base".to_string(), pr_base.clone()));
        }
        if let Some(pr_url) = &item.pr_url {
            line.push(("pr_url".to_string(), pr_url.clone()));
        }
        if let Some(actual_minutes) = item.actual_minutes {
            line.push(("actual_minutes".to_string(), actual_minutes.to_string()));
        }
        if let Some(changed_lines) = item.changed_lines {
            line.push(("changed_lines".to_string(), changed_lines.to_string()));
        }
        lines.push(line);
    }

    RunReport {
        ok: prepared
            .issues
            .iter()
            .all(|item| !matches!(item.status.as_str(), "blocked" | "split_required")),
        lines,
        progress_lines: Vec::new(),
    }
}

fn ensure_minimal_safe_repairs(config: &Config, github: &GitHubClient<'_>) -> Result<()> {
    github.ensure_managed_labels()?;
    let workdir = config.working_directory();
    git_ops::ensure_git_worktree(&workdir)?;
    Ok(())
}

fn prepare_branch(workdir: &Path, branch_name: &str, start_point: &str) -> Result<()> {
    if git_ops::local_branch_exists(workdir, branch_name)? {
        git_ops::delete_local_branch(workdir, branch_name, start_point)?;
    }
    git_ops::create_branch(workdir, branch_name, start_point)
}

fn preflight_target_repo(config: &Config) -> Result<String> {
    let workdir = config.working_directory();
    git_ops::ensure_git_worktree(&workdir)?;
    match git_ops::origin_repo_slug(&workdir)? {
        Some(remote) if remote == config.repo_slug() => Ok("true".to_string()),
        Some(_) => bail!("target_repo_mismatch"),
        None => Ok("unknown".to_string()),
    }
}

fn ensure_clean_preflight(config: &Config) -> Result<()> {
    git_ops::ensure_clean_worktree(&config.working_directory(), &[config.run_root()])?;
    Ok(())
}

fn nightly_branch_publication(
    parent_issue: u64,
    child_issue: u64,
    base_branch: &str,
    previous_success_branch: Option<&str>,
) -> Publication {
    Publication {
        branch_name: format!("nightloop/{}-{}", parent_issue, child_issue),
        pr_base: previous_success_branch.unwrap_or(base_branch).to_string(),
        push_before_pr: true,
    }
}

#[cfg(test)]
mod tests {
    use std::{env, fs, path::Path, process::Command};

    use super::{nightly_branch_publication, preflight_target_repo, Publication};
    use crate::config::Config;

    fn init_git_repo(path: &Path) {
        fs::create_dir_all(path).unwrap();
        assert!(Command::new("git")
            .current_dir(path)
            .args(["init", "-b", "main"])
            .output()
            .unwrap()
            .status
            .success());
    }

    fn config_for_repo(root: &Path, repo_root: &Path, remote: Option<&str>) -> Config {
        fs::create_dir_all(root).unwrap();
        if let Some(url) = remote {
            assert!(Command::new("git")
                .current_dir(repo_root)
                .args(["remote", "add", "origin", url])
                .output()
                .unwrap()
                .status
                .success());
        }
        let config_path = root.join("nightloop.toml");
        fs::write(
            &config_path,
            format!(
                r#"[github]
owner = "o"
repo = "r"
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
required_paths = ["README.md", "AGENTS.md"]

[estimation]
default_basis = "hybrid"
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
                repo_root.display()
            ),
        )
        .unwrap();
        Config::load(&config_path).unwrap()
    }

    #[test]
    fn preflight_target_repo_accepts_matching_remote_and_unknown_without_origin() {
        let root =
            env::temp_dir().join(format!("nightloop-runner-preflight-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let matching = root.join("matching");
        let unknown = root.join("unknown");
        init_git_repo(&matching);
        init_git_repo(&unknown);
        let matching_config = config_for_repo(
            &root.join("control-a"),
            &matching,
            Some("https://github.com/o/r.git"),
        );
        let unknown_config = config_for_repo(&root.join("control-b"), &unknown, None);
        assert_eq!(preflight_target_repo(&matching_config).unwrap(), "true");
        assert_eq!(preflight_target_repo(&unknown_config).unwrap(), "unknown");
    }

    #[test]
    fn nightly_branch_publication_is_stacked_and_always_pushes_before_pr() {
        let first: Publication = nightly_branch_publication(221, 222, "main", None);
        assert_eq!(first.branch_name, "nightloop/221-222");
        assert_eq!(first.pr_base, "main");
        assert!(first.push_before_pr);

        let second = nightly_branch_publication(221, 223, "main", Some("nightloop/221-222"));
        assert_eq!(second.pr_base, "nightloop/221-222");
        assert!(second.push_before_pr);
    }
}
