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
    for file in ["prd.md", "spec.md", "plan.md", "eval.md", "adr.md"] {
        fs::write(root.join("docs/templates").join(file), "template").unwrap();
    }
    for file in [
        "refine_prd.md",
        "refine_spec.md",
        "child_issue_from_plan.md",
        "estimate_issue.md",
    ] {
        fs::write(root.join("prompts").join(file), "prompt").unwrap();
    }
}

fn write_config(root: &Path) -> PathBuf {
    write_common_files(root);
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
            root.display(),
            root.join(".nightloop/history.jsonl").display()
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
