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
    for file in ["prd.md", "spec.md", "plan.md", "eval.md", "adr.md"] {
        fs::write(root.join("docs/templates").join(file), "template").unwrap();
    }
    for file in [
        "refine_prd.md",
        "refine_spec.md",
        "child_issue_from_plan.md",
        "estimate_issue.md",
        "plan_child_issue.md",
    ] {
        fs::write(root.join("prompts").join(file), "prompt").unwrap();
    }
}

fn write_target_config(control_root: &Path, name: &str, target_root: &Path) -> PathBuf {
    write_common_files(control_root);
    let targets_dir = control_root.join("targets");
    fs::create_dir_all(&targets_dir).unwrap();
    let config_path = targets_dir.join(format!("{name}.toml"));
    fs::write(
        &config_path,
        format!(
            r#"[github]
owner = "o"
repo = "r"
base_branch = "main"
request_copilot_review = false
copilot_reviewer = "github-copilot[bot]"

[agent]
command = "echo agent"
plan_command = "echo '{{\"model_profile\":\"balanced\",\"estimated_minutes\":65,\"confidence\":\"medium\",\"notes\":\"ok\"}}'"
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
            target_root.display()
        ),
    )
    .unwrap();
    config_path
}

fn write_config(root: &Path) -> PathBuf {
    write_config_for_target(root, root)
}

fn write_config_for_target(control_root: &Path, target_root: &Path) -> PathBuf {
    write_common_files(control_root);
    let config_path = control_root.join("nightloop.toml");
    fs::write(
        &config_path,
        format!(
            r#"[github]
owner = "o"
repo = "r"
base_branch = "main"
request_copilot_review = false
copilot_reviewer = "github-copilot[bot]"

[agent]
command = "echo agent"
plan_command = "echo '{{\"model_profile\":\"balanced\",\"estimated_minutes\":65,\"confidence\":\"medium\",\"notes\":\"ok\"}}'"
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
history_path = "{}"
min_samples_for_local = 1
local_weight = 0.65
template_weight = 0.35
"#,
            target_root.display(),
            target_root.join(".nightloop/history.jsonl").display()
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
            "## Background\none\n## Goal\ntwo\n## Scope\ndocs-only\n## Out of scope\nthree\n## Source of truth\n{}\n## Acceptance criteria\nfour\n## Verification\ncmd: cargo test\n## Dependencies\nnone\n## Target change size\nXS\n## Documentation impact\nreadme\n## Suggested model profile\nbalanced\n## Estimated execution time\n30\n## Estimation basis\ntemplate\n## Estimation confidence\nmedium\n",
            root.join("README.md").display()
        ),
    )
    .unwrap();
    issue_path
}

fn run_cli(root: &Path, args: &[&str]) -> (i32, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_nightloop"))
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    (
        output.status.code().unwrap_or(1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

#[test]
fn budget_command_outputs_compact_summary() {
    let root = temp_root("budget");
    let config = write_config(&root);
    let (code, stdout, stderr) = run_cli(
        &root,
        &[
            "--config",
            &config.display().to_string(),
            "budget",
            "--hours",
            "4",
        ],
    );
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(stdout.contains("fallback_slots=5"));
    assert!(stdout.contains("available_minutes=220"));
}

#[test]
fn lint_estimate_record_and_docs_commands_work_end_to_end() {
    let root = temp_root("core");
    let config = write_config(&root);
    let issue_path = write_issue(&root);

    let (lint_code, lint_stdout, lint_stderr) = run_cli(
        &root,
        &[
            "--config",
            &config.display().to_string(),
            "lint-issue",
            &issue_path.display().to_string(),
        ],
    );
    assert_eq!(lint_code, 0, "stderr={lint_stderr}");
    assert!(lint_stdout.contains("valid=true"));

    let (estimate_code, estimate_stdout, estimate_stderr) = run_cli(
        &root,
        &[
            "--config",
            &config.display().to_string(),
            "estimate-issue",
            &issue_path.display().to_string(),
            "--basis",
            "hybrid",
        ],
    );
    assert_eq!(estimate_code, 0, "stderr={estimate_stderr}");
    assert!(estimate_stdout.contains("model_profile=balanced"));
    assert!(estimate_stdout.contains("basis_used=template-fallback"));

    let record_path = root.join("record.json");
    fs::write(
        &record_path,
        format!(
            r#"{{
  "run_id": "run-1",
  "parent_issue": 10,
  "issue_number": 11,
  "issue_title": "Task",
  "model_profile": "balanced",
  "model": "gpt-5.4",
  "reasoning_effort": "medium",
  "target_size": "M",
  "docs_impact": "none",
  "estimated_minutes": 60,
  "actual_minutes": 55,
  "changed_lines": 120,
  "files_touched": 4,
  "success": true,
  "status": "success",
  "copilot_review": null,
  "branch": "nightloop/10-11",
  "pr_base": "main",
  "pr_url": null,
  "recorded_at": "2026-03-25T00:00:00Z"
}}"#
        ),
    )
    .unwrap();
    let (record_code, record_stdout, record_stderr) = run_cli(
        &root,
        &[
            "--config",
            &config.display().to_string(),
            "record-run",
            &record_path.display().to_string(),
        ],
    );
    assert_eq!(record_code, 0, "stderr={record_stderr}");
    assert!(record_stdout.contains("ok=true"));
    assert!(root.join(".nightloop/history.jsonl").exists());

    let (docs_code, docs_stdout, docs_stderr) = run_cli(
        &root,
        &["--config", &config.display().to_string(), "docs-check"],
    );
    assert_eq!(docs_code, 0, "stderr={docs_stderr}");
    assert!(docs_stdout.contains("ok=true"));
}

#[test]
fn lint_and_docs_use_target_repo_root_when_control_and_target_differ() {
    let root = temp_root("dual-root");
    let control = root.join("control");
    let target = root.join("target");
    fs::create_dir_all(&control).unwrap();
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("README.md"), "target readme").unwrap();
    fs::write(target.join("AGENTS.md"), "target agents").unwrap();

    let config = write_config_for_target(&control, &target);
    let issue_path = control.join("issue.md");
    fs::write(
        &issue_path,
        "## Background\none\n## Goal\ntwo\n## Scope\ndocs-only\n## Out of scope\nthree\n## Source of truth\nREADME.md\n## Acceptance criteria\nfour\n## Verification\ncmd: cargo test\n## Dependencies\nnone\n## Target change size\nXS\n## Documentation impact\nreadme\n## Suggested model profile\nbalanced\n## Estimated execution time\n30\n## Estimation basis\ntemplate\n## Estimation confidence\nmedium\n",
    )
    .unwrap();

    let (lint_code, lint_stdout, lint_stderr) = run_cli(
        &control,
        &[
            "--config",
            &config.display().to_string(),
            "lint-issue",
            &issue_path.display().to_string(),
        ],
    );
    assert_eq!(lint_code, 0, "stderr={lint_stderr}");
    assert!(lint_stdout.contains("valid=true"));

    let (docs_code, docs_stdout, docs_stderr) = run_cli(
        &control,
        &["--config", &config.display().to_string(), "docs-check"],
    );
    assert_eq!(docs_code, 0, "stderr={docs_stderr}");
    assert!(docs_stdout.contains("ok=true"));
}

#[test]
fn init_target_and_named_target_invocation_work() {
    let root = temp_root("named-target");
    let target = root.join("canaria");
    fs::create_dir_all(&target).unwrap();
    fs::create_dir_all(target.join("docs")).unwrap();
    fs::write(target.join("README.md"), "target readme").unwrap();
    fs::write(target.join("AGENTS.md"), "target agents").unwrap();
    write_common_files(&root);

    let (init_code, init_stdout, init_stderr) = run_cli(
        &root,
        &[
            "init-target",
            "--name",
            "canaria",
            "--repo",
            "UTAGEDA/canaria",
            "--workdir",
            &target.display().to_string(),
            "--agent-command",
            "codex exec --full-auto",
            "--plan-command",
            "codex exec --planner",
            "--default-model",
            "gpt-5.4-mini",
            "--default-reasoning-effort",
            "high",
            "--request-copilot-review",
        ],
    );
    assert_eq!(init_code, 0, "stderr={init_stderr}");
    assert!(init_stdout.contains("ok=true"));
    let named_config = root.join("targets/canaria.toml");
    assert!(named_config.exists());
    let named_contents = fs::read_to_string(&named_config).unwrap();
    assert!(named_contents.contains(r#"owner = "UTAGEDA""#));
    assert!(named_contents.contains(r#"repo = "canaria""#));
    assert!(named_contents.contains(&format!(r#"working_directory = "{}""#, target.display())));
    assert!(named_contents.contains(r#"command = "codex exec --full-auto""#));
    assert!(named_contents.contains(r#"plan_command = "codex exec --planner""#));
    assert!(named_contents.contains(r#"default_model = "gpt-5.4-mini""#));
    assert!(named_contents.contains(r#"default_reasoning_effort = "high""#));
    assert!(named_contents.contains(r#"request_copilot_review = true"#));
    assert!(init_stdout.contains(r#"agent_command="codex exec --full-auto""#));
    assert!(init_stdout.contains("request_copilot_review=true"));

    let issue_path = root.join("issue.md");
    fs::write(
        &issue_path,
        "## Background\none\n## Goal\ntwo\n## Scope\ndocs-only\n## Out of scope\nthree\n## Source of truth\nREADME.md\n## Acceptance criteria\nfour\n## Verification\ncmd: cargo test\n## Dependencies\nnone\n## Target change size\nXS\n## Documentation impact\nreadme\n## Suggested model profile\nbalanced\n## Estimated execution time\n30\n## Estimation basis\ntemplate\n## Estimation confidence\nmedium\n",
    )
    .unwrap();

    let (docs_code, docs_stdout, docs_stderr) =
        run_cli(&root, &["docs-check", "--target", "canaria"]);
    assert_eq!(docs_code, 0, "stderr={docs_stderr}");
    assert!(docs_stdout.contains("ok=true"));

    let (lint_code, lint_stdout, lint_stderr) = run_cli(
        &root,
        &[
            "lint-issue",
            "--target",
            "canaria",
            &issue_path.display().to_string(),
        ],
    );
    assert_eq!(lint_code, 0, "stderr={lint_stderr}");
    assert!(lint_stdout.contains("valid=true"));
}

#[test]
fn config_flag_overrides_named_target_and_help_mentions_target_workflow() {
    let root = temp_root("target-precedence");
    let control_target = root.join("target-a");
    let control_target_two = root.join("target-b");
    fs::create_dir_all(&control_target).unwrap();
    fs::create_dir_all(&control_target_two).unwrap();
    fs::write(control_target.join("README.md"), "readme").unwrap();
    fs::write(control_target.join("AGENTS.md"), "agents").unwrap();
    fs::write(control_target_two.join("README.md"), "readme").unwrap();
    fs::write(control_target_two.join("AGENTS.md"), "agents").unwrap();

    let explicit = write_config_for_target(&root, &control_target);
    let _named = write_target_config(&root, "canaria", &control_target_two);

    let issue_path = root.join("issue.md");
    fs::write(
        &issue_path,
        "## Background\none\n## Goal\ntwo\n## Scope\ndocs-only\n## Out of scope\nthree\n## Source of truth\nREADME.md\n## Acceptance criteria\nfour\n## Verification\ncmd: cargo test\n## Dependencies\nnone\n## Target change size\nXS\n## Documentation impact\nreadme\n## Suggested model profile\nbalanced\n## Estimated execution time\n30\n## Estimation basis\ntemplate\n## Estimation confidence\nmedium\n",
    )
    .unwrap();

    fs::remove_file(control_target_two.join("README.md")).unwrap();

    let (lint_code, lint_stdout, lint_stderr) = run_cli(
        &root,
        &[
            "--config",
            &explicit.display().to_string(),
            "--target",
            "canaria",
            "lint-issue",
            &issue_path.display().to_string(),
        ],
    );
    assert_eq!(lint_code, 0, "stderr={lint_stderr}");
    assert!(lint_stdout.contains("valid=true"));

    let (help_code, help_stdout, help_stderr) = run_cli(&root, &["--help"]);
    assert_eq!(help_code, 0, "stderr={help_stderr}");
    assert!(
        help_stdout.contains("nightloop init-target --name NAME --repo OWNER/REPO --workdir PATH")
    );
    assert!(help_stdout.contains("nightloop [--target NAME] setup-labels"));
    assert!(
        help_stdout.contains("nightloop [--target NAME] review-loop --parent ISSUE [--dry-run]")
    );
    assert!(help_stdout.contains("--agent-command CMD"));
    assert!(help_stdout.contains("--target NAME"));
}

#[test]
fn setup_labels_help_and_target_resolution_are_exposed() {
    let root = temp_root("setup-labels-help");
    let target = root.join("target");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("README.md"), "readme").unwrap();
    fs::write(target.join("AGENTS.md"), "agents").unwrap();
    let _named = write_target_config(&root, "canaria", &target);

    let (help_code, help_stdout, help_stderr) = run_cli(&root, &["setup-labels", "--help"]);
    assert_eq!(help_code, 0, "stderr={help_stderr}");
    assert!(help_stdout.contains("Usage: nightloop [--config PATH] [--target NAME] setup-labels"));

    let (missing_code, _missing_stdout, missing_stderr) =
        run_cli(&root, &["setup-labels", "--target", "missing"]);
    assert_ne!(missing_code, 0);
    assert!(missing_stderr.contains("target_config_not_found"));
}
