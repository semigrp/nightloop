use std::path::PathBuf;

use anyhow::Result;

use crate::config::Config;

const REQUIRED_TEMPLATE_FILES: &[&str] = &[
    "docs/templates/prd.md",
    "docs/templates/spec.md",
    "docs/templates/plan.md",
    "docs/templates/eval.md",
    "docs/templates/adr.md",
];

const REQUIRED_PROMPT_FILES: &[&str] = &[
    "prompts/refine_prd.md",
    "prompts/refine_spec.md",
    "prompts/child_issue_from_plan.md",
    "prompts/estimate_issue.md",
];

#[derive(Debug)]
pub struct MissingPath {
    pub kind: String,
    pub path: PathBuf,
}

#[derive(Debug)]
pub struct DocsReport {
    pub ok: bool,
    pub missing_paths: Vec<MissingPath>,
}

pub fn check_docs(config: &Config) -> Result<DocsReport> {
    let mut missing_paths = Vec::new();

    for path in &config.docs.required_paths {
        if !path.exists() {
            missing_paths.push(MissingPath {
                kind: "required_path".to_string(),
                path: path.clone(),
            });
        }
    }

    for path in REQUIRED_TEMPLATE_FILES {
        let path = PathBuf::from(path);
        if !path.exists() {
            missing_paths.push(MissingPath {
                kind: "template".to_string(),
                path,
            });
        }
    }

    for path in REQUIRED_PROMPT_FILES {
        let path = PathBuf::from(path);
        if !path.exists() {
            missing_paths.push(MissingPath {
                kind: "prompt".to_string(),
                path,
            });
        }
    }

    Ok(DocsReport {
        ok: missing_paths.is_empty(),
        missing_paths,
    })
}

#[cfg(test)]
mod tests {
    use std::{env, fs, path::PathBuf};

    use crate::config::Config;

    use super::check_docs;

    #[test]
    fn docs_check_reports_missing_prompt() {
        let root = env::temp_dir().join(format!("nightloop-docs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("docs/templates")).unwrap();
        fs::create_dir_all(root.join("prompts")).unwrap();
        fs::write(root.join("README.md"), "ok").unwrap();
        fs::write(root.join("AGENTS.md"), "ok").unwrap();
        for file in ["prd.md", "spec.md", "plan.md", "eval.md", "adr.md"] {
            fs::write(root.join("docs/templates").join(file), "ok").unwrap();
        }
        for file in [
            "refine_prd.md",
            "refine_spec.md",
            "child_issue_from_plan.md",
        ] {
            fs::write(root.join("prompts").join(file), "ok").unwrap();
        }
        let config_path = root.join("config.toml");
        fs::write(
            &config_path,
            r#"[github]
owner = "o"
repo = "r"
base_branch = "main"
request_copilot_review = false
copilot_reviewer = "github-copilot[bot]"

[agent]
command = "echo agent"
plan_command = "echo planner"
working_directory = "."
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
history_path = ".nightloop/history.jsonl"
min_samples_for_local = 1
local_weight = 0.65
template_weight = 0.35
"#,
        )
        .unwrap();
        let config = Config::load(&config_path).unwrap();
        let cwd = env::current_dir().unwrap();
        env::set_current_dir(&root).unwrap();
        let report = check_docs(&config).unwrap();
        env::set_current_dir(cwd).unwrap();
        assert!(!report.ok);
        assert!(report
            .missing_paths
            .iter()
            .any(|item| item.path == PathBuf::from("prompts/estimate_issue.md")));
    }
}
