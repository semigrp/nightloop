use std::{fs, path::Path};

use anyhow::{Context, Result};

const REQUIRED_SECTIONS: &[&str] = &[
    "## Background",
    "## Goal",
    "## Scope",
    "## Out of scope",
    "## Source of truth",
    "## Acceptance criteria",
    "## Verification",
    "## Dependencies",
    "## Target change size",
    "## Documentation impact",
    "## Suggested model profile",
    "## Estimated execution time",
    "## Estimation basis",
];

#[derive(Debug)]
pub struct LintReport {
    pub valid: bool,
    pub missing_sections: Vec<String>,
}

pub fn lint_markdown_issue(path: &Path) -> Result<LintReport> {
    let body = fs::read_to_string(path)
        .with_context(|| format!("failed to read issue markdown from {}", path.display()))?;

    let missing_sections = REQUIRED_SECTIONS
        .iter()
        .filter(|section| !body.contains(**section))
        .map(|s| s.to_string())
        .collect::<Vec<_>>();

    Ok(LintReport {
        valid: missing_sections.is_empty(),
        missing_sections,
    })
}
