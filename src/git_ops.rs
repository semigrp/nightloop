use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

use crate::{
    agent_exec,
    diff_budget::{self, DiffStat},
};

#[derive(Debug, Clone, Default)]
pub struct WorktreeStatus {
    pub dirty_paths: Vec<String>,
    pub ignored_only: bool,
    pub stderr: String,
}

#[derive(Debug, Clone, Default)]
pub struct GitFailureDetail {
    pub code: String,
    pub detail: Option<String>,
    pub git_stderr: Option<String>,
}

impl GitFailureDetail {
    fn new(code: &str) -> Self {
        Self {
            code: code.to_string(),
            detail: None,
            git_stderr: None,
        }
    }
}

pub fn worktree_status(workdir: &Path, ignored_paths: &[PathBuf]) -> Result<WorktreeStatus> {
    let command = build_status_command(workdir, ignored_paths);
    let result = agent_exec::run_shell_command(&command, workdir, &[], None)?;
    if !result.success() {
        bail!("git_status_failed");
    }
    let dirty_paths = result
        .stdout
        .lines()
        .filter_map(parse_status_path)
        .collect::<Vec<_>>();
    Ok(WorktreeStatus {
        dirty_paths,
        ignored_only: result.stdout.trim().is_empty(),
        stderr: result.stderr,
    })
}

pub fn ensure_clean_worktree(workdir: &Path, ignored_paths: &[PathBuf]) -> Result<()> {
    let status = worktree_status(workdir, ignored_paths)?;
    if !status.dirty_paths.is_empty() {
        bail!("git_worktree_dirty");
    }
    Ok(())
}

pub fn ensure_git_worktree(workdir: &Path) -> Result<()> {
    let result =
        agent_exec::run_shell_command("git rev-parse --is-inside-work-tree", workdir, &[], None)?;
    if !result.success() || result.stdout.trim() != "true" {
        bail!("target_repo_not_git_repo");
    }
    Ok(())
}

pub fn origin_repo_slug(workdir: &Path) -> Result<Option<String>> {
    let result = agent_exec::run_shell_command("git remote get-url origin", workdir, &[], None)?;
    if !result.success() {
        return Ok(None);
    }
    Ok(parse_origin_repo_slug(result.stdout.trim()))
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
    let failure = create_branch_detailed(workdir, branch, start_point)?;
    if let Some(failure) = failure {
        bail!(failure.code);
    }
    Ok(())
}

pub fn create_branch_detailed(
    workdir: &Path,
    branch: &str,
    start_point: &str,
) -> Result<Option<GitFailureDetail>> {
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
    if result.success() {
        return Ok(None);
    }
    let stderr = result.stderr;
    let mut failure = GitFailureDetail::new("git_branch_create_failed");
    failure.git_stderr = non_empty(stderr.clone());
    if stderr.contains("already exists") {
        failure.code = "git_branch_conflict".to_string();
        failure.detail = Some("branch_already_exists".to_string());
    }
    Ok(Some(failure))
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

pub fn local_branch_exists(workdir: &Path, branch: &str) -> Result<bool> {
    let result = agent_exec::run_shell_command(
        &format!(
            "git show-ref --verify --quiet {}",
            shell_quote(&format!("refs/heads/{branch}"))
        ),
        workdir,
        &[],
        None,
    )?;
    Ok(result.status_code == 0)
}

pub fn delete_local_branch(workdir: &Path, branch: &str, switch_to: &str) -> Result<()> {
    if current_branch(workdir)? == branch {
        switch_branch(workdir, switch_to)?;
    }
    let result = agent_exec::run_shell_command(
        &format!("git branch -D {}", shell_quote(branch)),
        workdir,
        &[],
        None,
    )?;
    if !result.success() {
        bail!("git_branch_delete_failed");
    }
    Ok(())
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

pub fn push_current_branch(workdir: &Path, branch: &str) -> Result<()> {
    let result = agent_exec::run_shell_command(
        &format!("git push --set-upstream origin {}", shell_quote(branch)),
        workdir,
        &[],
        None,
    )?;
    if !result.success() {
        bail!("git_push_failed");
    }
    Ok(())
}

pub fn diff_against(workdir: &Path, base_sha: &str, ignored_paths: &[PathBuf]) -> Result<DiffStat> {
    let mut command = format!("git diff --numstat {} -- .", shell_quote(base_sha));
    for ignored in ignored_paths {
        if let Ok(relative) = ignored.strip_prefix(workdir) {
            if !relative.as_os_str().is_empty() {
                let relative = relative.to_string_lossy().replace('\\', "/");
                command.push(' ');
                command.push_str(&shell_quote(&format!(":(exclude){relative}")));
            }
        }
    }
    let result = agent_exec::run_shell_command(&command, workdir, &[], None)?;
    if !result.success() {
        bail!("git_diff_failed");
    }
    diff_budget::parse_numstat(&result.stdout)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn build_status_command(workdir: &Path, ignored_paths: &[PathBuf]) -> String {
    let mut command = "git status --porcelain --untracked-files=all -- .".to_string();
    for ignored in ignored_paths {
        if let Ok(relative) = ignored.strip_prefix(workdir) {
            if !relative.as_os_str().is_empty() {
                let relative = relative.to_string_lossy().replace('\\', "/");
                command.push(' ');
                command.push_str(&shell_quote(&format!(":(exclude){relative}")));
            }
        }
    }
    command
}

fn parse_status_path(line: &str) -> Option<String> {
    let rest = line.get(3..)?.trim();
    let value = rest
        .split(" -> ")
        .last()
        .unwrap_or(rest)
        .trim_matches('"')
        .to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn non_empty(value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

pub fn parse_origin_repo_slug(url: &str) -> Option<String> {
    let trimmed = url.trim();
    if let Some(rest) = trimmed.strip_prefix("git@github.com:") {
        return Some(rest.trim_end_matches(".git").to_string());
    }
    if let Some(rest) = trimmed.strip_prefix("https://github.com/") {
        return Some(rest.trim_end_matches(".git").to_string());
    }
    if let Some(rest) = trimmed.strip_prefix("ssh://git@github.com/") {
        return Some(rest.trim_end_matches(".git").to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use std::{env, fs, path::Path, process::Command};

    use super::{
        diff_against, local_branch_exists, parse_origin_repo_slug, worktree_status, DiffStat,
    };

    fn git(dir: &Path, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn parses_origin_repo_slug_from_common_remote_urls() {
        assert_eq!(
            parse_origin_repo_slug("git@github.com:semigrp/nightloop.git"),
            Some("semigrp/nightloop".to_string())
        );
        assert_eq!(
            parse_origin_repo_slug("https://github.com/semigrp/nightloop.git"),
            Some("semigrp/nightloop".to_string())
        );
        assert_eq!(
            parse_origin_repo_slug("ssh://git@github.com/semigrp/nightloop.git"),
            Some("semigrp/nightloop".to_string())
        );
        assert_eq!(parse_origin_repo_slug("file:///tmp/repo"), None);
    }

    #[test]
    fn worktree_status_ignores_only_configured_run_root() {
        let root = env::temp_dir().join(format!("nightloop-gitops-ignore-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        git(&root, &["init"]);
        fs::write(root.join("tracked.txt"), "ok").unwrap();
        git(&root, &["add", "tracked.txt"]);
        git(
            &root,
            &[
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "init",
            ],
        );

        let run_root = root.join(".nightloop/runs");
        fs::create_dir_all(&run_root).unwrap();
        fs::write(run_root.join("artifact.log"), "log").unwrap();

        let status = worktree_status(&root, &[run_root]).unwrap();
        assert!(status.dirty_paths.is_empty());
        assert!(status.ignored_only);
    }

    #[test]
    fn worktree_status_still_reports_unrelated_dirty_paths() {
        let root = env::temp_dir().join(format!("nightloop-gitops-dirty-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        git(&root, &["init"]);
        fs::write(root.join("tracked.txt"), "ok").unwrap();
        git(&root, &["add", "tracked.txt"]);
        git(
            &root,
            &[
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "init",
            ],
        );

        let run_root = root.join(".nightloop/runs");
        fs::create_dir_all(&run_root).unwrap();
        fs::write(run_root.join("artifact.log"), "log").unwrap();
        fs::write(root.join("other.txt"), "dirty").unwrap();

        let status = worktree_status(&root, &[run_root]).unwrap();
        assert_eq!(status.dirty_paths, vec!["other.txt".to_string()]);
        assert!(!status.ignored_only);
    }

    #[test]
    fn detects_existing_local_branch() {
        let root = env::temp_dir().join(format!("nightloop-gitops-branch-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        git(&root, &["init"]);
        fs::write(root.join("tracked.txt"), "ok").unwrap();
        git(&root, &["add", "tracked.txt"]);
        git(
            &root,
            &[
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "init",
            ],
        );
        git(&root, &["branch", "nightloop/221-222"]);

        assert!(local_branch_exists(&root, "nightloop/221-222").unwrap());
        assert!(!local_branch_exists(&root, "nightloop/999-999").unwrap());
    }

    #[test]
    fn diff_against_counts_worktree_changes_and_ignores_run_root() {
        let root = env::temp_dir().join(format!("nightloop-gitops-diff-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        git(&root, &["init"]);
        fs::write(root.join("tracked.txt"), "one\n").unwrap();
        git(&root, &["add", "tracked.txt"]);
        git(
            &root,
            &[
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "init",
            ],
        );
        let base = Command::new("git")
            .current_dir(&root)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        let base = String::from_utf8_lossy(&base.stdout).trim().to_string();

        fs::write(root.join("tracked.txt"), "one\ntwo\n").unwrap();
        let run_root = root.join(".nightloop/runs");
        fs::create_dir_all(&run_root).unwrap();
        fs::write(run_root.join("artifact.log"), "ignore").unwrap();

        let stat = diff_against(&root, &base, &[run_root]).unwrap();
        assert_eq!(
            stat,
            DiffStat {
                changed_lines: 1,
                files_touched: 1,
            }
        );
    }
}
