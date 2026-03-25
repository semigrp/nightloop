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

impl PreparedIssue {
    fn repair(&mut self, repair: RepairAction) {
        if !self.repairs.contains(&repair) {
            self.repairs.push(repair);
        }
    }

    fn render_real_repair_lines(&self) -> Vec<Vec<(String, String)>> {
        self.repairs
            .iter()
            .filter_map(|repair| match repair.kind {
                RepairKind::ClearRunning
                | RepairKind::RestoreReadyFromBlocked
                | RepairKind::DeleteStaleBranch => {
                    let mut line = vec![
                        ("child_issue".to_string(), self.snapshot.number.to_string()),
                        ("repair".to_string(), repair.value.clone()),
                    ];
                    if let Some(branch) = &repair.branch {
                        line.push(("branch".to_string(), branch.clone()));
                    }
                    Some(line)
                }
                _ => None,
            })
            .collect()
    }

    fn render_dry_run_repair_lines(&self) -> Vec<Vec<(String, String)>> {
        self.repairs
            .iter()
            .filter_map(|repair| match repair.kind {
                RepairKind::WouldClearRunning
                | RepairKind::WouldRestoreReadyFromBlocked
                | RepairKind::WouldDeleteStaleBranch => {
                    let mut line = vec![
                        ("child_issue".to_string(), self.snapshot.number.to_string()),
                        (
                            "repair".to_string(),
                            match repair.kind {
                                RepairKind::WouldClearRunning => "would_clear_running",
                                RepairKind::WouldRestoreReadyFromBlocked => {
                                    "would_restore_ready_from_blocked"
                                }
                                RepairKind::WouldDeleteStaleBranch => "would_delete_stale_branch",
                                _ => unreachable!(),
                            }
                            .to_string(),
                        ),
                    ];
                    if let Some(branch) = &repair.branch {
                        line.push(("branch".to_string(), branch.clone()));
                    }
                    Some(line)
                }
                _ => None,
            })
            .collect()
    }
}

#[derive(Debug, Clone)]
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
    detail: Option<String>,
    git_stderr: Option<String>,
    gh_stderr: Option<String>,
    recovery: Option<String>,
    repairs: Vec<RepairAction>,
}

#[derive(Debug, Clone)]
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
    detail: Option<String>,
    git_stderr: Option<String>,
    gh_stderr: Option<String>,
    recovery: Option<String>,
    repair_lines: Vec<Vec<(String, String)>>,
    labels_repaired: u32,
    issues_repaired: u32,
    branches_repaired: u32,
}

#[derive(Debug, Clone, Default)]
struct ReviewLoopState {
    workflow: String,
    selected_child_issue: Option<u64>,
    plan_generated: bool,
    copilot_review_status: Option<String>,
    review_fix_rounds: u32,
    review_comments_total: u32,
    review_comments_applied: u32,
    review_comments_ignored: u32,
}

#[derive(Debug, Clone)]
struct SetupFailure {
    code: String,
    detail: Option<String>,
    git_stderr: Option<String>,
    gh_stderr: Option<String>,
    recovery: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RepairAction {
    kind: RepairKind,
    value: String,
    branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RepairKind {
    ClearRunning,
    WouldClearRunning,
    RestoreReadyFromBlocked,
    WouldRestoreReadyFromBlocked,
    DeleteStaleBranch,
    WouldDeleteStaleBranch,
}

impl SetupFailure {
    fn new(code: &str) -> Self {
        Self {
            code: code.to_string(),
            detail: None,
            git_stderr: None,
            gh_stderr: None,
            recovery: None,
        }
    }
}

fn repair_action(kind: RepairKind, value: &str, branch: Option<String>) -> RepairAction {
    RepairAction {
        kind,
        value: value.to_string(),
        branch,
    }
}

pub fn dry_run(config: &Config, parent_issue_number: u64, hours: u32) -> Result<RunReport> {
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
    collect_dry_run_repairs(config, &github, &mut prepared)?;
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
    if let Err(failure) = apply_real_run_repairs(config, &github, &mut prepared) {
        prepared.detail = failure.detail;
        prepared.git_stderr = failure.git_stderr;
        prepared.gh_stderr = failure.gh_stderr;
        prepared.recovery = failure.recovery;
        return Ok(build_report(&prepared, false, false, 0));
    }
    if let Err(failure) = ensure_clean_preflight(config) {
        prepared.detail = failure.detail;
        prepared.git_stderr = failure.git_stderr;
        prepared.gh_stderr = failure.gh_stderr;
        prepared.recovery = failure.recovery;
        return Ok(build_report(&prepared, false, false, 0));
    }
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

        let prompt = build_agent_prompt(&prepared.parent, child, estimate, config);
        let prompt_path = child_run_dir.join("agent-prompt.md");
        item.branch = Some(branch_name.clone());
        emit_progress(progress_implementing_branch(&branch_name));

        let mut labels_changed = false;
        if let Err(failure) = setup_issue_execution(
            config,
            &github,
            child.number,
            &workdir,
            &branch_name,
            &base_sha,
            &pr_base,
            &prompt_path,
            &prompt,
            &mut labels_changed,
            &mut item.repairs,
            &mut prepared.branches_repaired,
        ) {
            item.status = "skipped".to_string();
            item.reasons = vec![failure.code];
            item.detail = failure.detail;
            item.git_stderr = failure.git_stderr;
            item.gh_stderr = failure.gh_stderr;
            item.recovery = failure.recovery;
            ok = false;
            break;
        }
        prepared
            .repair_lines
            .extend(item.render_real_repair_lines());

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
            agent_exec::run_shell_command(&config.agent.command, &workdir, &envs, Some(&prompt))?;
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
            let clean = git_ops::ensure_clean_worktree(&workdir, &[config.run_root()]).is_ok();
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
            emit_progress(progress_requesting_copilot_review());
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
                workflow: "run".to_string(),
                planner_used: false,
                copilot_review: item.copilot_review.clone(),
                review_comments_total: 0,
                review_comments_applied: 0,
                review_comments_ignored: 0,
                fix_rounds: 0,
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

pub fn review_loop_dry_run(config: &Config, parent_issue_number: u64) -> Result<RunReport> {
    let github = GitHubClient::new(config);
    github.check_auth()?;
    let target_repo_match = preflight_target_repo(config)?;
    let mut prepared = prepare_campaign(
        config,
        &github,
        parent_issue_number,
        config.loop_cfg.min_hours,
        target_repo_match,
    )?;
    collect_dry_run_repairs(config, &github, &mut prepared)?;
    limit_to_first_selected(&mut prepared);
    let mut state = ReviewLoopState {
        workflow: "review_loop".to_string(),
        selected_child_issue: first_selected_issue(&prepared),
        plan_generated: first_selected_issue(&prepared).is_some(),
        copilot_review_status: first_selected_issue(&prepared).map(|_| "requested".to_string()),
        review_fix_rounds: if first_selected_issue(&prepared).is_some() {
            1
        } else {
            0
        },
        ..Default::default()
    };
    annotate_review_loop_dry_run(&mut prepared, &mut state);
    Ok(build_report_with_workflow(
        &prepared,
        true,
        true,
        0,
        Some(&state),
    ))
}

pub fn review_loop(config: &Config, parent_issue_number: u64) -> Result<RunReport> {
    let github = GitHubClient::new(config);
    github.check_auth()?;
    let target_repo_match = preflight_target_repo(config)?;
    let mut prepared = prepare_campaign(
        config,
        &github,
        parent_issue_number,
        config.loop_cfg.min_hours,
        target_repo_match,
    )?;
    let workdir = config.working_directory();
    if let Err(failure) = apply_real_run_repairs(config, &github, &mut prepared) {
        prepared.detail = failure.detail;
        prepared.git_stderr = failure.git_stderr;
        prepared.gh_stderr = failure.gh_stderr;
        prepared.recovery = failure.recovery;
        return Ok(build_report_with_workflow(
            &prepared,
            false,
            false,
            0,
            Some(&ReviewLoopState {
                workflow: "review_loop".to_string(),
                ..Default::default()
            }),
        ));
    }
    if let Err(failure) = ensure_clean_preflight(config) {
        prepared.detail = failure.detail;
        prepared.git_stderr = failure.git_stderr;
        prepared.gh_stderr = failure.gh_stderr;
        prepared.recovery = failure.recovery;
        return Ok(build_report_with_workflow(
            &prepared,
            false,
            false,
            0,
            Some(&ReviewLoopState {
                workflow: "review_loop".to_string(),
                ..Default::default()
            }),
        ));
    }
    git_ops::switch_branch(&workdir, &config.github.base_branch)?;
    limit_to_first_selected(&mut prepared);

    let selected_child = first_selected_issue(&prepared);
    let mut state = ReviewLoopState {
        workflow: "review_loop".to_string(),
        selected_child_issue: selected_child,
        ..Default::default()
    };
    if selected_child.is_none() {
        return Ok(build_report_with_workflow(
            &prepared,
            false,
            true,
            0,
            Some(&state),
        ));
    }

    let run_id = format!(
        "{}-parent-{}",
        Utc::now().format("%Y%m%dT%H%M%SZ"),
        parent_issue_number
    );
    let run_root = config.run_root().join(&run_id);
    fs::create_dir_all(&run_root)?;

    let item = prepared
        .issues
        .iter_mut()
        .find(|item| item.status == "selected")
        .ok_or_else(|| anyhow!("selected child missing"))?;
    let child = item
        .child
        .clone()
        .ok_or_else(|| anyhow!("selected issue missing parsed child"))?;
    let estimate = item
        .estimate
        .clone()
        .ok_or_else(|| anyhow!("selected issue missing estimate"))?;
    let child_run_dir = run_root.join(format!("child-{}", child.number));
    fs::create_dir_all(&child_run_dir)?;
    fs::write(
        child_run_dir.join("issue.json"),
        serde_json::to_string_pretty(&item.snapshot)?,
    )?;
    fs::write(child_run_dir.join("issue.md"), &item.snapshot.body)?;

    let branch_name = format!("nightloop/{}-{}", parent_issue_number, child.number);
    let pr_base = config.github.base_branch.clone();
    let base_sha = git_ops::rev_parse(&workdir, &config.github.base_branch)?;

    let plan_prompt = build_plan_prompt(&prepared.parent, &child, &estimate, config);
    let planner_prompt = apply_planner_prompt_prefix(config, &plan_prompt);
    emit_progress(progress_planning_child(child.number));
    let plan_result = agent_exec::run_shell_command(
        &config.agent.plan_command,
        &workdir,
        &[(
            "NIGHTLOOP_PROMPT_FILE".to_string(),
            child_run_dir.join("plan-prompt.md").display().to_string(),
        )],
        Some(&planner_prompt),
    )?;
    fs::write(child_run_dir.join("plan-prompt.md"), &planner_prompt)?;
    fs::write(child_run_dir.join("plan.stdout"), &plan_result.stdout)?;
    fs::write(child_run_dir.join("plan.stderr"), &plan_result.stderr)?;
    if !plan_result.success() {
        item.status = "skipped".to_string();
        item.reasons = vec!["plan_command_failed".to_string()];
        item.detail = Some("planner_command_failed".to_string());
        if !plan_result.stderr.trim().is_empty() {
            item.git_stderr = Some(plan_result.stderr.clone());
        }
        return Ok(build_report_with_workflow(
            &prepared,
            false,
            false,
            0,
            Some(&state),
        ));
    }
    let plan_text = if plan_result.stdout.trim().is_empty() {
        "No plan output produced.".to_string()
    } else {
        plan_result.stdout.clone()
    };
    fs::write(child_run_dir.join("plan.md"), &plan_text)?;
    state.plan_generated = true;
    item.detail = Some("plan_generated".to_string());

    let implementation_prompt = build_review_loop_implementation_prompt(
        &prepared.parent,
        &child,
        &estimate,
        config,
        &plan_text,
    );
    let actual_minutes = execute_single_child_flow(
        config,
        &github,
        &prepared.parent,
        &child,
        &estimate,
        item,
        &run_id,
        &child_run_dir,
        &implementation_prompt,
        &branch_name,
        &pr_base,
        &base_sha,
        &mut prepared.branches_repaired,
        true,
        false,
    )?;

    let pr_url = item
        .pr_url
        .clone()
        .ok_or_else(|| anyhow!("review loop expected pr url"))?;
    if item.copilot_review.is_none() {
        emit_progress(progress_requesting_copilot_review());
        match github.request_pr_review(&pr_url, &config.github.copilot_reviewer) {
            Ok(()) => item.copilot_review = Some("requested".to_string()),
            Err(_) => {
                item.copilot_review = Some("failed".to_string());
                state.copilot_review_status = Some("failed".to_string());
                item.reasons.push("gh_pr_review_request_failed".to_string());
                return Ok(build_report_with_workflow(
                    &prepared,
                    false,
                    false,
                    actual_minutes,
                    Some(&state),
                ));
            }
        }
    }
    let pr_number = github.pr_number_from_url(&pr_url)?;
    state.copilot_review_status = Some("requested".to_string());

    emit_progress(progress_waiting_for_copilot_review());
    let bundle = match github.poll_copilot_review(
        pr_number,
        config.review_loop.review_poll_interval_seconds,
        config.review_loop.review_wait_timeout_minutes,
    ) {
        Ok(bundle) => bundle,
        Err(err) if err.to_string() == "copilot_review_timeout" => {
            state.copilot_review_status = Some("timeout".to_string());
            item.reasons.push("copilot_review_timeout".to_string());
            return Ok(build_report_with_workflow(
                &prepared,
                false,
                false,
                actual_minutes,
                Some(&state),
            ));
        }
        Err(err) => return Err(err),
    };

    state.copilot_review_status = Some("received".to_string());
    state.review_comments_total = bundle.threads.len() as u32;
    fs::write(
        child_run_dir.join("copilot-review.json"),
        serde_json::to_string_pretty(&bundle.threads)?,
    )?;

    let fix_prompt = build_review_fix_prompt(
        &prepared.parent,
        &child,
        &estimate,
        config,
        &plan_text,
        &branch_name,
        &pr_base,
        &bundle,
    );
    emit_progress(progress_applying_review_feedback());
    let fix_result = agent_exec::run_shell_command(
        &config.agent.command,
        &workdir,
        &[(
            "NIGHTLOOP_PROMPT_FILE".to_string(),
            child_run_dir
                .join("review-fix-prompt.md")
                .display()
                .to_string(),
        )],
        Some(&fix_prompt),
    )?;
    fs::write(child_run_dir.join("review-fix-prompt.md"), &fix_prompt)?;
    fs::write(child_run_dir.join("review-fix.stdout"), &fix_result.stdout)?;
    fs::write(child_run_dir.join("review-fix.stderr"), &fix_result.stderr)?;
    state.review_fix_rounds = config.review_loop.review_max_fix_rounds.min(1);

    if !fix_result.success() {
        item.reasons.push("review_fix_command_failed".to_string());
        return Ok(build_report_with_workflow(
            &prepared,
            false,
            false,
            actual_minutes,
            Some(&state),
        ));
    }

    let fix_summary = summarize_review_fix_output(&fix_result.stdout, bundle.threads.len() as u32);
    state.review_comments_applied = fix_summary.0;
    state.review_comments_ignored = fix_summary.1;
    fs::write(
        child_run_dir.join("review-fix-summary.txt"),
        format!(
            "applied={}\nignored={}\n{}",
            fix_summary.0, fix_summary.1, fix_result.stdout
        ),
    )?;

    rerun_verification(config, &child, &workdir, &child_run_dir)?;
    let diff_stat = git_ops::diff_against(&workdir, &base_sha)?;
    diff_budget::enforce_diff_budget(config, &child, diff_stat)?;
    if diff_stat.changed_lines > 0 {
        git_ops::commit_all(
            &workdir,
            &format!(
                "nightloop: address Copilot review for #{} {}",
                child.number, child.title
            ),
        )?;
        git_ops::push_current_branch(&workdir, &branch_name)?;
    }

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
            workflow: "review_loop".to_string(),
            planner_used: true,
            copilot_review: state.copilot_review_status.clone(),
            review_comments_total: state.review_comments_total,
            review_comments_applied: state.review_comments_applied,
            review_comments_ignored: state.review_comments_ignored,
            fix_rounds: state.review_fix_rounds,
            branch: branch_name.clone(),
            pr_base: pr_base.clone(),
            pr_url: item.pr_url.clone(),
            recorded_at: Utc::now(),
        },
    )?;

    github.comment_issue(
        child.number,
        &format!(
            "nightloop review loop follow-up\n\n- copilot review status: received\n- review comments total: {}\n- applied comments: {}\n- ignored comments: {}",
            state.review_comments_total, state.review_comments_applied, state.review_comments_ignored
        ),
    )?;
    github.comment_issue(
        prepared.parent.number,
        &format!(
            "nightloop review loop summary\n\n- child: #{}\n- plan generated: true\n- copilot review: received\n- review fix rounds: 1\n- applied comments: {}\n- ignored comments: {}",
            child.number, state.review_comments_applied, state.review_comments_ignored
        ),
    )?;

    Ok(build_report_with_workflow(
        &prepared,
        false,
        true,
        actual_minutes,
        Some(&state),
    ))
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
            detail: None,
            git_stderr: None,
            gh_stderr: None,
            recovery: None,
            repairs: Vec::new(),
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
            .clone()
            .ok_or_else(|| anyhow!("valid lint report missing parsed child"))?;
        prepared.estimate = Some(estimate::estimate_child_issue(
            config,
            &child,
            default_basis,
        )?);
        prepared.reasons = selection::static_eligibility_reasons(config, &child);
        let reasons = prepared.reasons.clone();
        planned_issue_repairs(&reasons, &mut prepared);

        if prepared.reasons.is_empty()
            && !dependencies_satisfied_live(
                &child,
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
        detail: None,
        git_stderr: None,
        gh_stderr: None,
        recovery: None,
        repair_lines: Vec::new(),
        labels_repaired: 0,
        issues_repaired: 0,
        branches_repaired: 0,
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
            workflow: "run".to_string(),
            planner_used: false,
            copilot_review: None,
            review_comments_total: 0,
            review_comments_applied: 0,
            review_comments_ignored: 0,
            fix_rounds: 0,
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

fn expected_branch_name(prepared: &PreparedCampaign, issue_number: u64) -> String {
    format!("nightloop/{}-{}", prepared.parent.number, issue_number)
}

fn progress_planning_child(issue_number: u64) -> String {
    format!("planning child #{issue_number}")
}

fn progress_implementing_branch(branch: &str) -> String {
    format!("implementing branch {branch}")
}

fn progress_requesting_copilot_review() -> String {
    "requesting copilot review".to_string()
}

fn progress_waiting_for_copilot_review() -> String {
    "waiting for copilot review".to_string()
}

fn progress_applying_review_feedback() -> String {
    "applying review feedback".to_string()
}

fn emit_progress(message: String) {
    reporting::print_progress(&message);
}

fn build_nightly_progress_lines(prepared: &PreparedCampaign, dry_run: bool) -> Vec<String> {
    let mut lines = Vec::new();
    for item in &prepared.issues {
        let is_active =
            item.status == "selected" || item.status == "completed" || item.status == "blocked";
        if !is_active {
            continue;
        }
        let branch = item
            .branch
            .clone()
            .unwrap_or_else(|| expected_branch_name(prepared, item.snapshot.number));
        if dry_run {
            lines.push(format!("would {}", progress_implementing_branch(&branch)));
        }
    }
    lines
}

fn build_review_loop_progress_lines(
    prepared: &PreparedCampaign,
    dry_run: bool,
    workflow: &ReviewLoopState,
) -> Vec<String> {
    let Some(issue_number) = workflow.selected_child_issue else {
        return Vec::new();
    };
    let branch = prepared
        .issues
        .iter()
        .find(|item| item.snapshot.number == issue_number)
        .and_then(|item| item.branch.clone())
        .unwrap_or_else(|| expected_branch_name(prepared, issue_number));

    if dry_run {
        return vec![
            format!("would {}", progress_planning_child(issue_number)),
            format!("would {}", progress_implementing_branch(&branch)),
            format!("would {}", progress_requesting_copilot_review()),
            format!("would {}", progress_waiting_for_copilot_review()),
            format!("would {}", progress_applying_review_feedback()),
        ];
    }

    let mut lines = vec![progress_planning_child(issue_number)];
    let item = prepared
        .issues
        .iter()
        .find(|item| item.snapshot.number == issue_number);
    let planner_failed = item
        .map(|item| {
            item.reasons
                .iter()
                .any(|reason| reason == "plan_command_failed")
        })
        .unwrap_or(false);
    if !planner_failed {
        lines.push(progress_implementing_branch(&branch));
    }
    if matches!(
        workflow.copilot_review_status.as_deref(),
        Some("requested" | "received" | "timeout" | "failed")
    ) || item.and_then(|item| item.pr_url.as_ref()).is_some()
    {
        lines.push(progress_requesting_copilot_review());
    }
    if matches!(
        workflow.copilot_review_status.as_deref(),
        Some("received" | "timeout")
    ) {
        lines.push(progress_waiting_for_copilot_review());
    }
    if workflow.review_fix_rounds > 0
        || item
            .map(|item| {
                item.reasons
                    .iter()
                    .any(|reason| reason == "review_fix_command_failed")
            })
            .unwrap_or(false)
    {
        lines.push(progress_applying_review_feedback());
    }
    lines
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
        (
            "labels_repaired".to_string(),
            prepared.labels_repaired.to_string(),
        ),
        (
            "issues_repaired".to_string(),
            prepared.issues_repaired.to_string(),
        ),
        (
            "branches_repaired".to_string(),
            prepared.branches_repaired.to_string(),
        ),
    ]];
    if let Some(detail) = &prepared.detail {
        lines[0].push(("detail".to_string(), detail.clone()));
    }
    if let Some(git_stderr) = &prepared.git_stderr {
        lines[0].push(("git_stderr".to_string(), git_stderr.clone()));
    }
    if let Some(gh_stderr) = &prepared.gh_stderr {
        lines[0].push(("gh_stderr".to_string(), gh_stderr.clone()));
    }
    if let Some(recovery) = &prepared.recovery {
        lines[0].push(("recovery".to_string(), recovery.clone()));
    }
    lines.extend(prepared.repair_lines.iter().cloned());

    for item in &prepared.issues {
        let repair_lines = if dry_run {
            item.render_dry_run_repair_lines()
        } else {
            item.render_real_repair_lines()
        };
        lines.extend(repair_lines);
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
        if let Some(detail) = &item.detail {
            line.push(("detail".to_string(), detail.clone()));
        }
        if let Some(git_stderr) = &item.git_stderr {
            line.push(("git_stderr".to_string(), git_stderr.clone()));
        }
        if let Some(gh_stderr) = &item.gh_stderr {
            line.push(("gh_stderr".to_string(), gh_stderr.clone()));
        }
        if let Some(recovery) = &item.recovery {
            line.push(("recovery".to_string(), recovery.clone()));
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

    RunReport {
        ok,
        lines,
        progress_lines: if dry_run {
            build_nightly_progress_lines(prepared, dry_run)
        } else {
            Vec::new()
        },
    }
}

fn build_report_with_workflow(
    prepared: &PreparedCampaign,
    dry_run: bool,
    ok: bool,
    actual_total_minutes: u32,
    workflow: Option<&ReviewLoopState>,
) -> RunReport {
    let mut report = build_report(prepared, dry_run, ok, actual_total_minutes);
    if let Some(workflow) = workflow {
        report.lines[0].push(("workflow".to_string(), workflow.workflow.clone()));
        if let Some(issue) = workflow.selected_child_issue {
            report.lines[0].push(("selected_child_issue".to_string(), issue.to_string()));
        }
        report.lines[0].push((
            "plan_generated".to_string(),
            workflow.plan_generated.to_string(),
        ));
        if let Some(status) = &workflow.copilot_review_status {
            report.lines[0].push(("copilot_review".to_string(), status.clone()));
        }
        report.lines[0].push((
            "review_fix_rounds".to_string(),
            workflow.review_fix_rounds.to_string(),
        ));
        report.lines[0].push((
            "review_comments_total".to_string(),
            workflow.review_comments_total.to_string(),
        ));
        report.lines[0].push((
            "review_comments_applied".to_string(),
            workflow.review_comments_applied.to_string(),
        ));
        report.lines[0].push((
            "review_comments_ignored".to_string(),
            workflow.review_comments_ignored.to_string(),
        ));
        report.progress_lines = if dry_run {
            build_review_loop_progress_lines(prepared, dry_run, workflow)
        } else {
            Vec::new()
        };
    }
    report
}

fn annotate_review_loop_dry_run(_prepared: &mut PreparedCampaign, _state: &mut ReviewLoopState) {}

fn limit_to_first_selected(prepared: &mut PreparedCampaign) {
    let mut seen = false;
    for item in &mut prepared.issues {
        if item.status == "selected" && !seen {
            seen = true;
            continue;
        }
        if item.status == "selected" {
            item.status = "skipped".to_string();
            item.reasons = vec!["review_loop_single_child_only".to_string()];
        }
    }
    prepared.estimated_total_minutes = prepared
        .issues
        .iter()
        .filter(|item| item.status == "selected")
        .filter_map(|item| {
            item.estimate
                .as_ref()
                .map(|estimate| estimate.estimated_minutes)
        })
        .sum();
}

fn first_selected_issue(prepared: &PreparedCampaign) -> Option<u64> {
    prepared
        .issues
        .iter()
        .find(|item| item.status == "selected")
        .map(|item| item.snapshot.number)
}

fn build_plan_prompt(
    parent: &ParentIssue,
    child: &ChildIssue,
    estimate: &IssueEstimate,
    config: &Config,
) -> String {
    format!(
        "Create a concise implementation plan for GitHub child issue #{} from parent campaign #{}.\n\nParent issue: #{} {}\nChild issue: #{} {}\nSuggested model profile: {}\nExact model: {}\nReasoning effort: {}\n\nBackground:\n{}\n\nGoal:\n{}\n\nScope:\n{}\n\nOut of scope:\n{}\n\nSource of truth:\n{}\n\nAcceptance criteria:\n{}\n\nVerification:\n{}\n\nReturn a practical implementation plan in markdown.",
        child.number,
        parent.number,
        parent.number,
        parent.title,
        child.number,
        child.title,
        estimate.model_profile,
        estimate.model,
        estimate.reasoning_effort,
        child.background,
        child.goal,
        child.scope,
        child.out_of_scope,
        child.source_of_truth_raw,
        child.acceptance_criteria,
        child
            .verification
            .iter()
            .map(|command| command.command.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
    ) + &format!(
        "\n\nRepository guidance:\n- Working directory: {}\n- Base branch: {}",
        config.working_directory().display(),
        config.github.base_branch
    )
}

fn apply_planner_prompt_prefix(config: &Config, prompt: &str) -> String {
    let prefix = config.review_loop.planner_prompt_prefix.trim();
    if prefix.is_empty() {
        prompt.to_string()
    } else {
        format!("{prefix}\n\n{prompt}")
    }
}

fn build_review_loop_implementation_prompt(
    parent: &ParentIssue,
    child: &ChildIssue,
    estimate: &IssueEstimate,
    config: &Config,
    plan_text: &str,
) -> String {
    format!(
        "{}\n\nImplementation plan:\n{}\n\nFollow the implementation plan before writing code.",
        build_agent_prompt(parent, child, estimate, config),
        plan_text
    )
}

fn build_review_fix_prompt(
    parent: &ParentIssue,
    child: &ChildIssue,
    estimate: &IssueEstimate,
    config: &Config,
    plan_text: &str,
    branch_name: &str,
    pr_base: &str,
    bundle: &crate::github::CopilotReviewBundle,
) -> String {
    let comments = bundle
        .threads
        .iter()
        .map(|thread| match (&thread.path, thread.line) {
            (Some(path), Some(line)) => format!("- {}:{} {}", path, line, thread.body),
            _ => format!("- {}", thread.body),
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Address Copilot review feedback for child issue #{}.\n\nParent issue: #{} {}\nChild issue: #{} {}\nModel profile: {}\nModel: {}\nReasoning effort: {}\nBranch: {}\nPR base: {}\n\nSaved implementation plan:\n{}\n\nCopilot review comments:\n{}\n\nApply only valid, in-scope comments. Ignore invalid, out-of-scope, non-actionable, or already-satisfied comments. At the end, output a short summary with sections: Applied comments, Ignored comments, Ignored reasons.",
        child.number,
        parent.number,
        parent.title,
        child.number,
        child.title,
        estimate.model_profile,
        estimate.model,
        estimate.reasoning_effort,
        branch_name,
        pr_base,
        plan_text,
        comments
    ) + &format!(
        "\n\nRepository guidance:\n- Working directory: {}\n- Base branch: {}",
        config.working_directory().display(),
        config.github.base_branch
    )
}

fn rerun_verification(
    config: &Config,
    child: &ChildIssue,
    workdir: &std::path::Path,
    child_run_dir: &std::path::Path,
) -> Result<()> {
    for (index, command) in child.verification.iter().enumerate() {
        let result = agent_exec::run_shell_command(&command.command, workdir, &[], None)?;
        fs::write(
            child_run_dir.join(format!("review-verification-{:02}.stdout", index + 1)),
            &result.stdout,
        )?;
        fs::write(
            child_run_dir.join(format!("review-verification-{:02}.stderr", index + 1)),
            &result.stderr,
        )?;
        if !result.success() {
            anyhow::bail!("verification_failed");
        }
    }
    let _ = config;
    Ok(())
}

fn summarize_review_fix_output(output: &str, total_comments: u32) -> (u32, u32) {
    let applied = output
        .lines()
        .filter(|line| line.to_ascii_lowercase().contains("applied"))
        .count() as u32;
    let applied = applied.min(total_comments);
    (applied, total_comments.saturating_sub(applied))
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

fn ensure_clean_preflight(config: &Config) -> Result<(), SetupFailure> {
    let workdir = config.working_directory();
    let run_root = config.run_root();
    let status = git_ops::worktree_status(&workdir, &[run_root]).map_err(|_| {
        let mut failure = SetupFailure::new("git_status_failed");
        failure.recovery = Some("inspect_git_status".to_string());
        failure
    })?;
    if !status.dirty_paths.is_empty() {
        let mut failure = SetupFailure::new("git_worktree_dirty");
        failure.detail = Some(status.dirty_paths.join(","));
        failure.git_stderr = if status.stderr.trim().is_empty() {
            None
        } else {
            Some(status.stderr)
        };
        failure.recovery = Some("clean_worktree_and_retry".to_string());
        return Err(failure);
    }
    Ok(())
}

fn planned_issue_repairs(reasons: &[String], prepared: &mut PreparedIssue) {
    let had_running = reasons
        .iter()
        .any(|reason| reason == "label_present_running");
    let had_blocked = reasons
        .iter()
        .any(|reason| reason == "label_present_blocked");

    if had_running {
        prepared.repair(repair_action(
            RepairKind::WouldClearRunning,
            "cleared_running",
            None,
        ));
        prepared.recovery = Some("auto_clear_running".to_string());
    }
    if had_blocked {
        prepared.repair(repair_action(
            RepairKind::WouldRestoreReadyFromBlocked,
            "restored_ready_from_blocked",
            None,
        ));
        prepared.recovery = Some("auto_restore_ready".to_string());
    }
    if had_running || had_blocked {
        prepared.reasons.retain(|reason| {
            reason != "label_present_running"
                && reason != "label_present_blocked"
                && reason != "label_missing_ready"
        });
    }
}

fn collect_dry_run_repairs(
    config: &Config,
    github: &GitHubClient<'_>,
    prepared: &mut PreparedCampaign,
) -> Result<()> {
    let statuses = github.list_labels()?;
    let existing = statuses.into_iter().collect::<HashSet<_>>();
    for item in crate::github::reconcile_managed_labels(&existing, &config.labels)
        .into_iter()
        .filter(|item| item.status == "created")
    {
        prepared.labels_repaired += 1;
        prepared.repair_lines.push(vec![
            ("label".to_string(), item.name),
            ("status".to_string(), "would_create".to_string()),
        ]);
    }
    mark_branch_repairs(config, prepared)?;
    for item in &prepared.issues {
        for repair in &item.repairs {
            match repair.kind {
                RepairKind::WouldDeleteStaleBranch => prepared.branches_repaired += 1,
                RepairKind::WouldClearRunning | RepairKind::WouldRestoreReadyFromBlocked => {
                    prepared.issues_repaired += 1
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn apply_real_run_repairs(
    config: &Config,
    github: &GitHubClient<'_>,
    prepared: &mut PreparedCampaign,
) -> Result<(), SetupFailure> {
    let label_statuses = github.ensure_managed_labels().map_err(|err| SetupFailure {
        code: "gh_label_create_failed".to_string(),
        detail: Some("managed_label_bootstrap_failed".to_string()),
        git_stderr: None,
        gh_stderr: Some(err.to_string()),
        recovery: Some("run_setup_labels_and_retry".to_string()),
    })?;
    for item in label_statuses
        .into_iter()
        .filter(|item| item.status == "created")
    {
        prepared.labels_repaired += 1;
        prepared.repair_lines.push(vec![
            ("label".to_string(), item.name),
            ("status".to_string(), "created".to_string()),
        ]);
    }

    for item in &mut prepared.issues {
        if item.status != "selected" {
            continue;
        }
        let has_running = item
            .repairs
            .iter()
            .any(|repair| repair.kind == RepairKind::WouldClearRunning);
        let has_blocked = item
            .repairs
            .iter()
            .any(|repair| repair.kind == RepairKind::WouldRestoreReadyFromBlocked);
        if !has_running && !has_blocked {
            continue;
        }

        let mut remove = Vec::new();
        if has_running {
            remove.push(config.labels.running.as_str());
        }
        if has_blocked {
            remove.push(config.labels.blocked.as_str());
        }
        let add = [config.labels.ready.as_str()];
        github
            .normalize_issue_labels(item.snapshot.number, &remove, &add)
            .map_err(|err| SetupFailure {
                code: "gh_label_update_failed".to_string(),
                detail: Some("issue_label_repair_failed".to_string()),
                git_stderr: None,
                gh_stderr: Some(err.to_string()),
                recovery: Some("repair_issue_labels_and_retry".to_string()),
            })?;
        if has_running {
            item.repair(repair_action(
                RepairKind::ClearRunning,
                "cleared_running",
                None,
            ));
            prepared.issues_repaired += 1;
        }
        if has_blocked {
            item.repair(repair_action(
                RepairKind::RestoreReadyFromBlocked,
                "restored_ready_from_blocked",
                None,
            ));
            prepared.issues_repaired += 1;
        }
    }

    mark_branch_repairs(config, prepared).map_err(|err| SetupFailure {
        code: "git_branch_lookup_failed".to_string(),
        detail: Some("stale_branch_scan_failed".to_string()),
        git_stderr: Some(err.to_string()),
        gh_stderr: None,
        recovery: Some("inspect_local_branches".to_string()),
    })?;
    Ok(())
}

fn mark_branch_repairs(config: &Config, prepared: &mut PreparedCampaign) -> Result<()> {
    let workdir = config.working_directory();
    for item in &mut prepared.issues {
        if item.status != "selected" {
            continue;
        }
        let branch_name = format!(
            "nightloop/{}-{}",
            prepared.parent.number, item.snapshot.number
        );
        if git_ops::local_branch_exists(&workdir, &branch_name)? {
            item.repair(repair_action(
                RepairKind::WouldDeleteStaleBranch,
                "deleted_stale_branch",
                Some(branch_name.clone()),
            ));
            item.recovery = Some("auto_delete_stale_branch".to_string());
        }
    }
    Ok(())
}

fn setup_issue_execution(
    config: &Config,
    github: &GitHubClient<'_>,
    child_number: u64,
    workdir: &std::path::Path,
    branch_name: &str,
    base_sha: &str,
    restore_branch: &str,
    prompt_path: &std::path::Path,
    prompt: &str,
    labels_changed: &mut bool,
    repairs: &mut Vec<RepairAction>,
    branches_repaired: &mut u32,
) -> Result<(), SetupFailure> {
    if git_ops::local_branch_exists(workdir, branch_name).map_err(|_| {
        let mut failure = SetupFailure::new("git_branch_lookup_failed");
        failure.recovery = Some("inspect_local_branches".to_string());
        failure
    })? {
        git_ops::delete_local_branch(workdir, branch_name, restore_branch).map_err(|_| {
            let mut failure = SetupFailure::new("git_branch_delete_failed");
            failure.detail = Some("branch_already_exists".to_string());
            failure.recovery = Some("inspect_stale_branch_and_retry".to_string());
            failure
        })?;
        repairs.push(repair_action(
            RepairKind::DeleteStaleBranch,
            "deleted_stale_branch",
            Some(branch_name.to_string()),
        ));
        *branches_repaired += 1;
    }

    if let Some(branch_failure) = git_ops::create_branch_detailed(workdir, branch_name, base_sha)
        .map_err(|_| {
            let mut failure = SetupFailure::new("git_branch_create_failed");
            failure.recovery = Some("inspect_git_error_and_retry".to_string());
            failure
        })?
    {
        return Err(SetupFailure {
            code: branch_failure.code,
            detail: branch_failure.detail,
            git_stderr: branch_failure.git_stderr,
            gh_stderr: None,
            recovery: Some("delete_branch_and_retry".to_string()),
        });
    }

    if let Err(err) =
        github.remove_labels(child_number, &[&config.labels.ready, &config.labels.review])
    {
        return Err(setup_failure_after_branch(
            config,
            github,
            child_number,
            workdir,
            restore_branch,
            branch_name,
            labels_changed,
            SetupFailure {
                code: "gh_label_update_failed".to_string(),
                detail: Some("remove_ready_review_failed".to_string()),
                git_stderr: None,
                gh_stderr: Some(err.to_string()),
                recovery: Some("restore_issue_labels_and_retry".to_string()),
            },
        ));
    }
    *labels_changed = true;
    if let Err(err) = github.add_labels(child_number, &[&config.labels.running]) {
        return Err(setup_failure_after_branch(
            config,
            github,
            child_number,
            workdir,
            restore_branch,
            branch_name,
            labels_changed,
            SetupFailure {
                code: "gh_label_update_failed".to_string(),
                detail: Some("add_running_failed".to_string()),
                git_stderr: None,
                gh_stderr: Some(err.to_string()),
                recovery: Some("restore_issue_labels_and_retry".to_string()),
            },
        ));
    }

    if let Err(err) = fs::write(prompt_path, prompt) {
        return Err(setup_failure_after_branch(
            config,
            github,
            child_number,
            workdir,
            restore_branch,
            branch_name,
            labels_changed,
            SetupFailure {
                code: "prompt_write_failed".to_string(),
                detail: None,
                git_stderr: Some(err.to_string()),
                gh_stderr: None,
                recovery: Some("fix_filesystem_and_retry".to_string()),
            },
        ));
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn execute_single_child_flow(
    config: &Config,
    github: &GitHubClient<'_>,
    parent: &ParentIssue,
    child: &ChildIssue,
    estimate: &IssueEstimate,
    item: &mut PreparedIssue,
    run_id: &str,
    child_run_dir: &std::path::Path,
    prompt: &str,
    branch_name: &str,
    pr_base: &str,
    base_sha: &str,
    branches_repaired: &mut u32,
    push_after_commit: bool,
    record_telemetry: bool,
) -> Result<u32> {
    let workdir = config.working_directory();
    let prompt_path = child_run_dir.join("agent-prompt.md");
    item.branch = Some(branch_name.to_string());
    emit_progress(progress_implementing_branch(branch_name));

    let mut labels_changed = false;
    setup_issue_execution(
        config,
        github,
        child.number,
        &workdir,
        branch_name,
        base_sha,
        pr_base,
        &prompt_path,
        prompt,
        &mut labels_changed,
        &mut item.repairs,
        branches_repaired,
    )
    .map_err(|failure| anyhow!(failure.code))?;
    fs::write(&prompt_path, prompt)?;

    let min_diff = config.diff.min_lines.max(child.target_size.min_lines());
    let max_diff = config.diff.max_lines.min(child.target_size.max_lines());
    let envs = vec![
        (
            "NIGHTLOOP_PARENT_ISSUE".to_string(),
            parent.number.to_string(),
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
        ("NIGHTLOOP_BASE_BRANCH".to_string(), pr_base.to_string()),
        ("NIGHTLOOP_DIFF_MIN".to_string(), min_diff.to_string()),
        ("NIGHTLOOP_DIFF_MAX".to_string(), max_diff.to_string()),
    ];

    let started = Instant::now();
    let agent_result =
        agent_exec::run_shell_command(&config.agent.command, &workdir, &envs, Some(prompt))?;
    fs::write(child_run_dir.join("agent.stdout"), &agent_result.stdout)?;
    fs::write(child_run_dir.join("agent.stderr"), &agent_result.stderr)?;

    let mut failure_code = None;
    if !agent_result.success() {
        failure_code = Some("agent_command_failed".to_string());
    }
    if failure_code.is_none() {
        for (index, command) in child.verification.iter().enumerate() {
            let result = agent_exec::run_shell_command(&command.command, &workdir, &envs, None)?;
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

    let diff_stat = git_ops::diff_against(&workdir, base_sha).unwrap_or(DiffStat {
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
    if let Some(code) = failure_code {
        finalize_failure(
            config,
            github,
            child,
            estimate,
            parent.number,
            run_id,
            branch_name,
            pr_base,
            &code,
            diff_stat,
            actual_minutes,
        )?;
        item.status = "blocked".to_string();
        item.reasons = vec![code];
        return Ok(actual_minutes);
    }

    git_ops::commit_all(
        &workdir,
        &format!("nightloop: issue #{} {}", child.number, child.title),
    )?;
    if push_after_commit {
        git_ops::push_current_branch(&workdir, branch_name)?;
    }
    let pr_url = github.create_draft_pr(
        pr_base,
        branch_name,
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
        emit_progress(progress_requesting_copilot_review());
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
    if record_telemetry {
        telemetry::append_run_record(
            &config.telemetry_history_path(),
            &RunRecord {
                run_id: run_id.to_string(),
                parent_issue: parent.number,
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
                workflow: "run".to_string(),
                planner_used: false,
                copilot_review: item.copilot_review.clone(),
                review_comments_total: 0,
                review_comments_applied: 0,
                review_comments_ignored: 0,
                fix_rounds: 0,
                branch: branch_name.to_string(),
                pr_base: pr_base.to_string(),
                pr_url: Some(pr_url.clone()),
                recorded_at: Utc::now(),
            },
        )?;
    }
    item.status = "completed".to_string();
    Ok(actual_minutes)
}

fn setup_failure_after_branch(
    config: &Config,
    github: &GitHubClient<'_>,
    child_number: u64,
    workdir: &std::path::Path,
    restore_branch: &str,
    created_branch: &str,
    labels_changed: &mut bool,
    mut failure: SetupFailure,
) -> SetupFailure {
    if *labels_changed {
        if let Err(err) = github.remove_labels(child_number, &[&config.labels.running]) {
            if failure.gh_stderr.is_none() {
                failure.gh_stderr = Some(err.to_string());
            }
        }
        if let Err(err) = github.add_labels(child_number, &[&config.labels.ready]) {
            if failure.gh_stderr.is_none() {
                failure.gh_stderr = Some(err.to_string());
            }
        }
    }
    if let Err(err) = git_ops::switch_branch(workdir, restore_branch) {
        if failure.git_stderr.is_none() {
            failure.git_stderr = Some(err.to_string());
        }
    }
    if let Ok(true) = git_ops::local_branch_exists(workdir, created_branch) {
        if let Err(err) = git_ops::delete_local_branch(workdir, created_branch, restore_branch) {
            if failure.git_stderr.is_none() {
                failure.git_stderr = Some(err.to_string());
            }
        }
    }
    if failure.detail.is_none() {
        failure.detail = Some(format!("branch={created_branch}"));
    }
    failure
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
    use std::{collections::HashMap, env, fs, process::Command};

    use crate::{
        config::Config,
        models::{IssueSnapshot, IssueState, ParentIssue},
    };

    use super::{
        apply_planner_prompt_prefix, build_copilot_review_comment_line, build_report,
        build_report_with_workflow, preflight_target_repo, progress_applying_review_feedback,
        progress_implementing_branch, progress_planning_child, progress_requesting_copilot_review,
        progress_waiting_for_copilot_review, repair_action, PreparedCampaign, PreparedIssue,
        RepairKind, ReviewLoopState,
    };

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

    #[test]
    fn planner_prompt_prefix_is_applied_only_to_plan_prompt() {
        let root = env::temp_dir().join(format!("nightloop-plan-prefix-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let config = config_for_target(&root, &root, "semigrp/nightloop");
        let prompt = apply_planner_prompt_prefix(&config, "body");
        assert!(prompt.starts_with("/plan\n\n"));
        assert!(prompt.ends_with("body"));
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

    #[test]
    fn shell_command_succeeds_when_prompt_is_sent_on_stdin() {
        let root = env::temp_dir().join(format!("nightloop-runner-stdin-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let result = crate::agent_exec::run_shell_command(
            "python3 -c 'import sys; data=sys.stdin.read(); print(\"stdin-ok\" if data else \"stdin-missing\"); raise SystemExit(0 if data else 1)'",
            &root,
            &[(
                "NIGHTLOOP_PROMPT_FILE".to_string(),
                root.join("prompt.md").display().to_string(),
            )],
            Some("prompt body"),
        )
        .unwrap();
        assert!(result.success());
        assert_eq!(result.stdout, "stdin-ok");
    }

    #[test]
    fn dry_run_repairs_report_running_label_and_counts() {
        let campaign = PreparedCampaign {
            parent: ParentIssue {
                number: 221,
                title: "Parent".to_string(),
                body: String::new(),
                state: IssueState::Open,
                labels: Vec::new(),
                url: None,
                sections: Default::default(),
                children: Vec::new(),
            },
            issues: vec![PreparedIssue {
                snapshot: IssueSnapshot {
                    number: 222,
                    title: "Child".to_string(),
                    body: String::new(),
                    state: IssueState::Open,
                    labels: vec!["agent:running".to_string()],
                    url: None,
                },
                child: None,
                estimate: None,
                reasons: vec!["label_present_running".to_string()],
                lint_findings: Vec::new(),
                status: "skipped".to_string(),
                actual_minutes: None,
                branch: None,
                pr_url: None,
                copilot_review: None,
                detail: None,
                git_stderr: None,
                gh_stderr: None,
                recovery: None,
                repairs: vec![
                    repair_action(RepairKind::WouldClearRunning, "cleared_running", None),
                    repair_action(
                        RepairKind::WouldDeleteStaleBranch,
                        "deleted_stale_branch",
                        Some("nightloop/221-222".to_string()),
                    ),
                ],
            }],
            issue_snapshots: HashMap::new(),
            dependency_done_cache: HashMap::new(),
            estimated_total_minutes: 0,
            remaining_minutes: 100,
            hours: 2,
            target_repo_match: "true".to_string(),
            target_repo_root: "/tmp/target".to_string(),
            run_root: "/tmp/target/.nightloop/runs".to_string(),
            detail: None,
            git_stderr: None,
            gh_stderr: None,
            recovery: None,
            repair_lines: vec![vec![
                ("label".to_string(), "agent:running".to_string()),
                ("status".to_string(), "would_create".to_string()),
            ]],
            labels_repaired: 1,
            issues_repaired: 1,
            branches_repaired: 1,
        };
        let report = build_report(&campaign, true, true, 0);
        assert!(report.ok);
        assert!(report.lines[0]
            .iter()
            .any(|(key, value)| key == "issues_repaired" && value == "1"));
        assert!(report.lines[1].iter().any(|(key, _value)| key == "label"));
        assert!(report.lines.iter().any(|line| line
            .iter()
            .any(|(key, value)| key == "repair" && value == "would_clear_running")));
        assert!(report.lines.iter().any(|line| line
            .iter()
            .any(|(key, value)| key == "repair" && value == "would_delete_stale_branch")));
        assert!(report.progress_lines.is_empty());
    }

    #[test]
    fn nightly_report_emits_branch_progress_for_selected_issue() {
        let campaign = PreparedCampaign {
            parent: ParentIssue {
                number: 221,
                title: "Parent".to_string(),
                body: String::new(),
                state: IssueState::Open,
                labels: Vec::new(),
                url: None,
                sections: Default::default(),
                children: Vec::new(),
            },
            issues: vec![PreparedIssue {
                snapshot: IssueSnapshot {
                    number: 222,
                    title: "Child".to_string(),
                    body: String::new(),
                    state: IssueState::Open,
                    labels: vec!["agent:ready".to_string()],
                    url: None,
                },
                child: None,
                estimate: None,
                reasons: Vec::new(),
                lint_findings: Vec::new(),
                status: "selected".to_string(),
                actual_minutes: None,
                branch: None,
                pr_url: None,
                copilot_review: None,
                detail: None,
                git_stderr: None,
                gh_stderr: None,
                recovery: None,
                repairs: Vec::new(),
            }],
            issue_snapshots: HashMap::new(),
            dependency_done_cache: HashMap::new(),
            estimated_total_minutes: 80,
            remaining_minutes: 20,
            hours: 2,
            target_repo_match: "true".to_string(),
            target_repo_root: "/tmp/target".to_string(),
            run_root: "/tmp/target/.nightloop/runs".to_string(),
            detail: None,
            git_stderr: None,
            gh_stderr: None,
            recovery: None,
            repair_lines: Vec::new(),
            labels_repaired: 0,
            issues_repaired: 0,
            branches_repaired: 0,
        };
        let report = build_report(&campaign, true, true, 0);
        assert_eq!(
            report.progress_lines,
            vec![format!(
                "would {}",
                progress_implementing_branch("nightloop/221-222")
            )]
        );
        let real_report = build_report(&campaign, false, true, 0);
        assert!(real_report.progress_lines.is_empty());
    }

    #[test]
    fn real_review_loop_report_no_longer_carries_progress_lines() {
        let campaign = PreparedCampaign {
            parent: ParentIssue {
                number: 221,
                title: "Parent".to_string(),
                body: String::new(),
                state: IssueState::Open,
                labels: Vec::new(),
                url: None,
                sections: Default::default(),
                children: Vec::new(),
            },
            issues: vec![PreparedIssue {
                snapshot: IssueSnapshot {
                    number: 222,
                    title: "Child".to_string(),
                    body: String::new(),
                    state: IssueState::Open,
                    labels: vec!["agent:ready".to_string()],
                    url: None,
                },
                child: None,
                estimate: None,
                reasons: Vec::new(),
                lint_findings: Vec::new(),
                status: "completed".to_string(),
                actual_minutes: Some(5),
                branch: Some("nightloop/221-222".to_string()),
                pr_url: Some("https://example.com/pr/1".to_string()),
                copilot_review: Some("requested".to_string()),
                detail: None,
                git_stderr: None,
                gh_stderr: None,
                recovery: None,
                repairs: Vec::new(),
            }],
            issue_snapshots: HashMap::new(),
            dependency_done_cache: HashMap::new(),
            estimated_total_minutes: 80,
            remaining_minutes: 20,
            hours: 2,
            target_repo_match: "true".to_string(),
            target_repo_root: "/tmp/target".to_string(),
            run_root: "/tmp/target/.nightloop/runs".to_string(),
            detail: None,
            git_stderr: None,
            gh_stderr: None,
            recovery: None,
            repair_lines: Vec::new(),
            labels_repaired: 0,
            issues_repaired: 0,
            branches_repaired: 0,
        };
        let workflow = ReviewLoopState {
            workflow: "review_loop".to_string(),
            selected_child_issue: Some(222),
            plan_generated: true,
            copilot_review_status: Some("received".to_string()),
            review_fix_rounds: 1,
            review_comments_total: 2,
            review_comments_applied: 1,
            review_comments_ignored: 1,
        };
        let report = build_report_with_workflow(&campaign, false, true, 5, Some(&workflow));
        assert_eq!(report.progress_lines, Vec::<String>::new());
    }

    #[test]
    fn progress_helpers_define_review_loop_live_phase_order() {
        assert_eq!(
            vec![
                progress_planning_child(222),
                progress_implementing_branch("nightloop/221-222"),
                progress_requesting_copilot_review(),
                progress_waiting_for_copilot_review(),
                progress_applying_review_feedback(),
            ],
            vec![
                "planning child #222".to_string(),
                "implementing branch nightloop/221-222".to_string(),
                "requesting copilot review".to_string(),
                "waiting for copilot review".to_string(),
                "applying review feedback".to_string(),
            ]
        );
    }
}
