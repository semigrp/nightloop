use std::{fs, path::Path};

use anyhow::{Context, Result};

use crate::{
    config::Config,
    issue_parse,
    models::{ChildIssue, IssueSnapshot, IssueState, SourceRefKind},
};

#[derive(Debug, Clone)]
pub struct LintFinding {
    pub code: String,
    pub field: Option<String>,
    pub message: String,
}

#[derive(Debug)]
pub struct LintReport {
    pub valid: bool,
    pub findings: Vec<LintFinding>,
    pub child: Option<ChildIssue>,
}

pub fn lint_markdown_issue(config: &Config, path: &Path) -> Result<LintReport> {
    let body = fs::read_to_string(path)
        .with_context(|| format!("failed to read issue markdown from {}", path.display()))?;
    let title = path
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "local-issue".to_string());
    let snapshot = IssueSnapshot {
        number: 0,
        title,
        body,
        state: IssueState::Open,
        labels: Vec::new(),
        url: None,
    };
    Ok(lint_child_issue(config, &snapshot))
}

pub fn lint_child_issue(config: &Config, snapshot: &IssueSnapshot) -> LintReport {
    let sections = issue_parse::parse_sections(&snapshot.body);
    let mut findings = Vec::new();

    for (key, title) in issue_parse::REQUIRED_CHILD_SECTIONS {
        if sections
            .get(*key)
            .map(|value| value.trim().is_empty())
            .unwrap_or(true)
        {
            findings.push(finding(
                "missing_section",
                Some((*key).to_string()),
                format!("missing required section: {title}"),
            ));
        }
    }

    let verification_raw = sections.get("verification").cloned().unwrap_or_default();
    if !verification_raw.is_empty()
        && issue_parse::parse_verification_commands(&verification_raw).is_empty()
    {
        findings.push(finding(
            "verification_empty",
            Some("verification".to_string()),
            "verification section must contain cmd: lines or a fenced sh/bash/shell block"
                .to_string(),
        ));
    }

    if let Some(target_size) = sections.get("target change size") {
        match crate::models::SizeBand::from_text(target_size) {
            Some(size_band) => {
                if size_band.max_lines() > config.diff.max_lines {
                    findings.push(finding(
                        "size_band_exceeds_diff_max",
                        Some("target change size".to_string()),
                        format!(
                            "target size {} exceeds configured max diff {}",
                            size_band.as_str(),
                            config.diff.max_lines
                        ),
                    ));
                }
                if size_band.min_lines() < config.diff.min_lines {
                    findings.push(finding(
                        "size_band_below_diff_min",
                        Some("target change size".to_string()),
                        format!(
                            "target size {} starts below configured min diff {}",
                            size_band.as_str(),
                            config.diff.min_lines
                        ),
                    ));
                }
            }
            None => findings.push(finding(
                "invalid_target_size",
                Some("target change size".to_string()),
                "target change size must be one of XS, S, M, or L".to_string(),
            )),
        }
    }

    if let Some(value) = sections.get("documentation impact") {
        if crate::models::DocsImpact::from_text(value).is_none() {
            findings.push(finding(
                "invalid_docs_impact",
                Some("documentation impact".to_string()),
                "documentation impact must be none, readme, user-facing-docs, or architecture-docs"
                    .to_string(),
            ));
        }
    }

    if let Some(value) = sections.get("estimation basis") {
        if crate::models::EstimationBasis::from_text(value).is_none() {
            findings.push(finding(
                "invalid_estimation_basis",
                Some("estimation basis".to_string()),
                "estimation basis must be template, local, hybrid, ai, or manual".to_string(),
            ));
        }
    }

    if let Some(value) = sections.get("estimation confidence") {
        if crate::models::Confidence::from_text(value).is_none() {
            findings.push(finding(
                "invalid_estimation_confidence",
                Some("estimation confidence".to_string()),
                "estimation confidence must be low, medium, or high".to_string(),
            ));
        }
    }

    if let Some(value) = sections.get("estimated execution time") {
        match value.trim().parse::<u32>() {
            Ok(minutes) if minutes > 0 => {}
            _ => findings.push(finding(
                "invalid_estimated_minutes",
                Some("estimated execution time".to_string()),
                "estimated execution time must be a positive integer".to_string(),
            )),
        }
    }

    if let Some(value) = sections.get("suggested model profile") {
        if value.trim().is_empty() {
            findings.push(finding(
                "invalid_model_profile",
                Some("suggested model profile".to_string()),
                "suggested model profile must not be empty".to_string(),
            ));
        } else if config.model_profile(value.trim()).is_none() {
            findings.push(finding(
                "unknown_model_profile",
                Some("suggested model profile".to_string()),
                format!("suggested model profile {} is not configured", value.trim()),
            ));
        }
    }

    if let Some(value) = sections.get("dependencies") {
        if let Err(err) = issue_parse::parse_dependencies(value) {
            findings.push(finding(
                "invalid_dependency",
                Some("dependencies".to_string()),
                err.to_string(),
            ));
        }
    }

    if let Some(value) = sections.get("source of truth") {
        match issue_parse::parse_source_refs(value) {
            Ok(refs) => {
                for source in refs {
                    match source.kind {
                        SourceRefKind::RepoRelative { path } | SourceRefKind::Absolute { path } => {
                            if !path.exists() {
                                findings.push(finding(
                                    "source_path_missing",
                                    Some("source of truth".to_string()),
                                    format!("missing source-of-truth path {}", path.display()),
                                ));
                            }
                        }
                        SourceRefKind::Url { .. } => {}
                    }
                }
            }
            Err(err) => findings.push(finding(
                "invalid_source_ref",
                Some("source of truth".to_string()),
                err.to_string(),
            )),
        }
    }

    let child = if findings.is_empty() {
        match issue_parse::build_child_issue(snapshot, sections) {
            Ok(child) => Some(child),
            Err(err) => {
                findings.push(finding("parse_failed", None, err.to_string()));
                None
            }
        }
    } else {
        None
    };

    LintReport {
        valid: findings.is_empty(),
        findings,
        child,
    }
}

fn finding(code: &str, field: Option<String>, message: String) -> LintFinding {
    LintFinding {
        code: code.to_string(),
        field,
        message,
    }
}

#[cfg(test)]
mod tests {
    use std::{env, fs};

    use crate::config::Config;

    use super::lint_markdown_issue;

    fn test_config(root: &std::path::Path) -> Config {
        let toml = format!(
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
required_paths = ["README.md"]

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
            root.join("history.jsonl").display()
        );
        let path = root.join("config.toml");
        fs::write(&path, toml).unwrap();
        Config::load(&path).unwrap()
    }

    #[test]
    fn lint_valid_issue_passes() {
        let root = env::temp_dir().join(format!("nightloop-lint-valid-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let readme_path = root.join("README.md");
        fs::write(&readme_path, "ok").unwrap();
        let issue_path = root.join("issue.md");
        fs::write(
            &issue_path,
            format!(
                "## Background\none\n## Goal\ntwo\n## Scope\ndocs-only\n## Out of scope\nthree\n## Source of truth\n{}\n## Acceptance criteria\nfour\n## Verification\ncmd: cargo test\n## Dependencies\nnone\n## Target change size\nXS\n## Documentation impact\nreadme\n## Suggested model profile\nbalanced\n## Estimated execution time\n30\n## Estimation basis\ntemplate\n## Estimation confidence\nmedium\n",
                readme_path.display()
            ),
        )
        .unwrap();
        let report = lint_markdown_issue(&test_config(&root), &issue_path).unwrap();
        assert!(report.valid);
    }

    #[test]
    fn lint_invalid_issue_reports_multiple_findings() {
        let root = env::temp_dir().join(format!("nightloop-lint-invalid-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let issue_path = root.join("issue.md");
        fs::write(
            &issue_path,
            "## Background\none\n## Goal\ntwo\n## Verification\nnothing here\n## Dependencies\nabc\n## Documentation impact\nwrong\n",
        )
        .unwrap();
        let report = lint_markdown_issue(&test_config(&root), &issue_path).unwrap();
        assert!(!report.valid);
        assert!(report
            .findings
            .iter()
            .any(|finding| finding.code == "missing_section"));
        assert!(report
            .findings
            .iter()
            .any(|finding| finding.code == "verification_empty"));
    }
}
