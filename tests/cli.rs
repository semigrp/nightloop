use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn temp_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "nightloop-cli-{}-{}-{}",
        name,
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

fn write_common_files(root: &Path) {
    fs::create_dir_all(root.join("docs/templates")).unwrap();
    fs::create_dir_all(root.join("prompts")).unwrap();
    fs::write(root.join("README.md"), "readme").unwrap();
    fs::write(root.join("AGENTS.md"), "agents").unwrap();
    fs::write(
        root.join("nightloop.example.toml"),
        include_str!("../nightloop.example.toml"),
    )
    .unwrap();
    fs::write(root.join("docs/templates/plan.md"), "template").unwrap();
    fs::write(root.join("prompts/plan_child_issue.md"), "prompt").unwrap();
}

fn write_config_for_target(
    control_root: &Path,
    target_root: &Path,
    agent_command: &str,
) -> PathBuf {
    write_common_files(control_root);
    let config_path = control_root.join("nightloop.toml");
    fs::write(
        &config_path,
        format!(
            r#"[github]
owner = "o"
repo = "r"
base_branch = "main"

[agent]
command = "{}"
plan_command = "printf 'plan ok\n'"
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
            agent_command.replace('"', "\\\""),
            target_root.display()
        ),
    )
    .unwrap();
    config_path
}

fn write_issue(root: &Path) -> PathBuf {
    let issue_path = root.join("issue.md");
    fs::write(
        &issue_path,
        format!(
            "## Background\none\n## Goal\ntwo\n## Scope\ndocs-only\n## Out of scope\nthree\n## Source of truth\n{}\n## Acceptance criteria\nfour\n## Verification\ncmd: git status --short\n## Dependencies\nnone\n## Target change size\nXS\n## Documentation impact\nreadme\n## Suggested model profile\nbalanced\n## Estimated execution time\n30\n## Estimation basis\ntemplate\n## Estimation confidence\nmedium\n",
            root.join("README.md").display()
        ),
    )
    .unwrap();
    issue_path
}

fn run_cli(root: &Path, args: &[&str], path_prefix: Option<&Path>) -> (i32, String, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nightloop"));
    command.current_dir(root).args(args);
    if let Some(prefix) = path_prefix {
        let existing = std::env::var("PATH").unwrap_or_default();
        command.env("PATH", format!("{}:{}", prefix.display(), existing));
    }
    let output = command.output().unwrap();
    (
        output.status.code().unwrap_or(1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

fn init_git_repo(path: &Path) {
    fs::create_dir_all(path).unwrap();
    let output = Command::new("git")
        .current_dir(path)
        .args(["init", "-b", "main"])
        .output()
        .unwrap();
    assert!(output.status.success());
    Command::new("git")
        .current_dir(path)
        .args(["config", "user.name", "Nightloop Test"])
        .output()
        .unwrap();
    Command::new("git")
        .current_dir(path)
        .args(["config", "user.email", "nightloop@example.com"])
        .output()
        .unwrap();
}

fn commit_file(repo: &Path, file: &str, contents: &str) {
    fs::write(repo.join(file), contents).unwrap();
    assert!(Command::new("git")
        .current_dir(repo)
        .args(["add", file])
        .output()
        .unwrap()
        .status
        .success());
    assert!(Command::new("git")
        .current_dir(repo)
        .args(["commit", "-m", "init"])
        .output()
        .unwrap()
        .status
        .success());
}

fn write_mock_gh(bin_dir: &Path, state_dir: &Path) {
    fs::create_dir_all(bin_dir).unwrap();
    fs::create_dir_all(state_dir.join("issues")).unwrap();
    fs::write(state_dir.join("labels.txt"), "campaign\n").unwrap();
    let script = format!(
        r#"#!/bin/sh
set -eu
STATE_DIR="{}"
if [ "$1" = "auth" ] && [ "$2" = "status" ]; then
  exit 0
fi
if [ "$1" = "label" ] && [ "$2" = "list" ]; then
  awk 'NF {{ printf "{{\"name\":\"%s\"}}\n", $0 }}' "$STATE_DIR/labels.txt" | paste -sd, - | sed 's/^/[/' | sed 's/$/]/'
  exit 0
fi
if [ "$1" = "label" ] && [ "$2" = "create" ]; then
  printf "%s\n" "$3" | tr -d "'" >> "$STATE_DIR/labels.txt"
  exit 0
fi
if [ "$1" = "issue" ] && [ "$2" = "view" ]; then
  cat "$STATE_DIR/issues/$3.json"
  exit 0
fi
if [ "$1" = "issue" ] && [ "$2" = "edit" ]; then
  exit 0
fi
if [ "$1" = "issue" ] && [ "$2" = "comment" ]; then
  exit 0
fi
if [ "$1" = "pr" ] && [ "$2" = "create" ]; then
  printf "https://example.test/pr/1\n"
  exit 0
fi
echo "unsupported gh invocation: $@" >&2
exit 1
"#,
        state_dir.display()
    );
    let gh_path = bin_dir.join("gh");
    fs::write(&gh_path, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&gh_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&gh_path, perms).unwrap();
    }
}

fn write_issue_snapshot(state_dir: &Path, number: u64, title: &str, body: &str, labels: &[&str]) {
    let labels_json = labels
        .iter()
        .map(|label| format!(r#"{{"name":"{}"}}"#, label))
        .collect::<Vec<_>>()
        .join(",");
    fs::write(
        state_dir.join("issues").join(format!("{number}.json")),
        format!(
            r#"{{"number":{},"title":"{}","body":{},"state":"OPEN","labels":[{}],"url":"https://example.test/issues/{}"}}"#,
            number,
            title,
            serde_json::to_string(body).unwrap(),
            labels_json,
            number
        ),
    )
    .unwrap();
}

#[test]
fn check_lint_and_estimate_commands_work_end_to_end() {
    let root = temp_root("core");
    let target = root.join("target");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("README.md"), "target readme").unwrap();
    fs::write(target.join("AGENTS.md"), "target agents").unwrap();
    let config = write_config_for_target(&root, &target, "printf '\\nagent\\n' >> README.md");
    let issue_path = write_issue(&target);

    let bin = root.join("bin");
    let state = root.join("gh-state");
    write_mock_gh(&bin, &state);

    let (check_code, check_stdout, check_stderr) = run_cli(
        &root,
        &["--config", &config.display().to_string(), "check"],
        Some(&bin),
    );
    assert_eq!(check_code, 0, "stderr={check_stderr}");
    assert!(check_stdout.contains("ok=true"));
    assert!(check_stdout.contains("labels_created="));

    let (lint_code, lint_stdout, lint_stderr) = run_cli(
        &root,
        &[
            "--config",
            &config.display().to_string(),
            "lint",
            &issue_path.display().to_string(),
        ],
        Some(&bin),
    );
    assert_eq!(lint_code, 0, "stderr={lint_stderr}");
    assert!(lint_stdout.contains("valid=true"));

    let (estimate_code, estimate_stdout, estimate_stderr) = run_cli(
        &root,
        &[
            "--config",
            &config.display().to_string(),
            "estimate",
            &issue_path.display().to_string(),
            "--basis",
            "hybrid",
        ],
        Some(&bin),
    );
    assert_eq!(estimate_code, 0, "stderr={estimate_stderr}");
    assert!(estimate_stdout.contains("basis_used=template-fallback"));
    assert!(!estimate_stdout.contains("ai_estimated_minutes"));
}

#[test]
fn estimate_rejects_removed_ai_basis() {
    let root = temp_root("estimate-ai");
    let target = root.join("target");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("README.md"), "target readme").unwrap();
    fs::write(target.join("AGENTS.md"), "target agents").unwrap();
    let config = write_config_for_target(&root, &target, "true");
    let issue_path = write_issue(&target);
    let (code, _stdout, stderr) = run_cli(
        &root,
        &[
            "--config",
            &config.display().to_string(),
            "estimate",
            &issue_path.display().to_string(),
            "--basis",
            "ai",
        ],
        None,
    );
    assert_ne!(code, 0);
    assert!(stderr.contains("invalid estimate basis"));
}

#[test]
fn init_emits_minimal_target_config() {
    let root = temp_root("init");
    let target = root.join("repo");
    fs::create_dir_all(&target).unwrap();
    write_common_files(&root);
    let (code, stdout, stderr) = run_cli(
        &root,
        &[
            "init",
            "canaria",
            "UTAGEDA/canaria",
            &target.display().to_string(),
        ],
        None,
    );
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(stdout.contains("ok=true"));
    let named_contents = fs::read_to_string(root.join("targets/canaria.toml")).unwrap();
    assert!(!named_contents.contains("request_copilot_review"));
    assert!(!named_contents.contains("[review_loop]"));
    assert!(named_contents.contains(r#"required_paths = ["README.md", "AGENTS.md"]"#));
}

#[test]
fn help_mentions_only_surviving_commands_and_removed_commands_fail() {
    let root = temp_root("help");
    let (help_code, help_stdout, help_stderr) = run_cli(&root, &["--help"], None);
    assert_eq!(help_code, 0, "stderr={help_stderr}");
    assert!(help_stdout.contains("nightloop init NAME OWNER/REPO WORKDIR"));
    assert!(help_stdout.contains("nightloop check"));
    assert!(help_stdout.contains("nightloop lint PATH"));
    assert!(help_stdout.contains("nightloop estimate PATH --basis template|local|hybrid"));
    assert!(help_stdout.contains("nightloop start PARENT_ISSUE"));
    assert!(help_stdout.contains("nightloop nightly PARENT_ISSUE --hours 2|3|4|5|6"));
    assert!(!help_stdout.contains("setup-labels"));
    assert!(!help_stdout.contains("record-run"));
    assert!(!help_stdout.contains("Compatibility aliases"));

    for removed in [
        "setup-labels",
        "budget",
        "record-run",
        "review-loop",
        "run",
        "lint-issue",
    ] {
        let (code, _stdout, stderr) = run_cli(&root, &[removed], None);
        assert_ne!(code, 0, "removed command unexpectedly succeeded: {removed}");
        assert!(stderr.contains("unknown command"));
    }
}

#[test]
fn start_runs_single_child_and_creates_draft_pr() {
    let root = temp_root("start");
    let control = root.join("control");
    let repo = root.join("repo");
    let bare = root.join("remote.git");
    fs::create_dir_all(&control).unwrap();
    init_git_repo(&repo);
    assert!(Command::new("git")
        .current_dir(&root)
        .args(["init", "--bare", bare.display().to_string().as_str()])
        .output()
        .unwrap()
        .status
        .success());
    assert!(Command::new("git")
        .current_dir(&repo)
        .args([
            "remote",
            "add",
            "origin",
            bare.display().to_string().as_str()
        ])
        .output()
        .unwrap()
        .status
        .success());
    commit_file(&repo, "README.md", "seed\n");
    fs::write(repo.join("AGENTS.md"), "agents\n").unwrap();
    assert!(Command::new("git")
        .current_dir(&repo)
        .args(["add", "AGENTS.md"])
        .output()
        .unwrap()
        .status
        .success());
    assert!(Command::new("git")
        .current_dir(&repo)
        .args(["commit", "-m", "add agents"])
        .output()
        .unwrap()
        .status
        .success());

    let config =
        write_config_for_target(&control, &repo, "printf '\\nagent change\\n' >> README.md");
    let bin = root.join("bin");
    let state = root.join("gh-state");
    write_mock_gh(&bin, &state);

    let parent_body = "## Ordered child Issues\n- [ ] #222 first child\n";
    let child_body = "## Background\none\n## Goal\ntwo\n## Scope\ndocs-only\n## Out of scope\nthree\n## Source of truth\nREADME.md\n## Acceptance criteria\nfour\n## Verification\ncmd: git status --short\n## Dependencies\nnone\n## Target change size\nXS\n## Documentation impact\nreadme\n## Suggested model profile\nbalanced\n## Estimated execution time\n30\n## Estimation basis\ntemplate\n## Estimation confidence\nmedium\n";
    write_issue_snapshot(&state, 221, "Parent", parent_body, &["campaign"]);
    write_issue_snapshot(
        &state,
        222,
        "Child",
        child_body,
        &["night-run", "agent:ready"],
    );

    let (code, stdout, stderr) = run_cli(
        &control,
        &["--config", &config.display().to_string(), "start", "221"],
        Some(&bin),
    );
    assert_eq!(code, 0, "stdout={stdout}\nstderr={stderr}");
    assert!(stdout.contains("child_issue=222"));
    assert!(stdout.contains("status=success"));
    assert!(stdout.contains("pr_url=https://example.test/pr/1"));
    assert!(repo.join(".nightloop/history.jsonl").exists());
}
