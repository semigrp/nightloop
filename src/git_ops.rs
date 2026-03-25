use std::path::Path;

use anyhow::{bail, Result};

use crate::{
    agent_exec,
    diff_budget::{self, DiffStat},
};

pub fn ensure_clean_worktree(workdir: &Path) -> Result<()> {
    let result = agent_exec::run_shell_command("git status --porcelain", workdir, &[], None)?;
    if !result.success() {
        bail!("git_status_failed");
    }
    if !result.stdout.trim().is_empty() {
        bail!("git_worktree_dirty");
    }
    Ok(())
}

pub fn switch_branch(workdir: &Path, branch: &str) -> Result<()> {
    let result = agent_exec::run_shell_command(
        &format!("git switch {}", shell_quote(branch)),
        workdir,
        &[],
        None,
    )?;
    if !result.success() {
        bail!("git_switch_failed");
    }
    Ok(())
}

pub fn create_branch(workdir: &Path, branch: &str, start_point: &str) -> Result<()> {
    let result = agent_exec::run_shell_command(
        &format!(
            "git switch -c {} {}",
            shell_quote(branch),
            shell_quote(start_point)
        ),
        workdir,
        &[],
        None,
    )?;
    if !result.success() {
        bail!("git_branch_create_failed");
    }
    Ok(())
}

pub fn rev_parse(workdir: &Path, value: &str) -> Result<String> {
    let result = agent_exec::run_shell_command(
        &format!("git rev-parse {}", shell_quote(value)),
        workdir,
        &[],
        None,
    )?;
    if !result.success() {
        bail!("git_rev_parse_failed");
    }
    Ok(result
        .stdout
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string())
}

pub fn current_branch(workdir: &Path) -> Result<String> {
    let result = agent_exec::run_shell_command("git branch --show-current", workdir, &[], None)?;
    if !result.success() {
        bail!("git_branch_current_failed");
    }
    Ok(result.stdout.trim().to_string())
}

pub fn commit_all(workdir: &Path, message: &str) -> Result<()> {
    let add = agent_exec::run_shell_command("git add -A", workdir, &[], None)?;
    if !add.success() {
        bail!("git_add_failed");
    }
    let commit = agent_exec::run_shell_command(
        &format!("git commit -m {}", shell_quote(message)),
        workdir,
        &[],
        None,
    )?;
    if !commit.success() {
        bail!("git_commit_failed");
    }
    Ok(())
}

pub fn diff_against(workdir: &Path, base_sha: &str) -> Result<DiffStat> {
    let result = agent_exec::run_shell_command(
        &format!("git diff --numstat {} HEAD", shell_quote(base_sha)),
        workdir,
        &[],
        None,
    )?;
    if !result.success() {
        bail!("git_diff_failed");
    }
    diff_budget::parse_numstat(&result.stdout)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
