use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{anyhow, bail, Result};

use crate::models::{
    ChildIssue, Confidence, DocsImpact, EstimationBasis, IssueSnapshot, ParentChildRef,
    ParentIssue, SizeBand, SourceRef, SourceRefKind, VerificationCommand, VerificationSource,
};

pub const REQUIRED_CHILD_SECTIONS: &[(&str, &str)] = &[
    ("background", "Background"),
    ("goal", "Goal"),
    ("scope", "Scope"),
    ("out of scope", "Out of scope"),
    ("source of truth", "Source of truth"),
    ("acceptance criteria", "Acceptance criteria"),
    ("verification", "Verification"),
    ("dependencies", "Dependencies"),
    ("target change size", "Target change size"),
    ("documentation impact", "Documentation impact"),
    ("suggested model profile", "Suggested model profile"),
    ("estimated execution time", "Estimated execution time"),
    ("estimation basis", "Estimation basis"),
    ("estimation confidence", "Estimation confidence"),
];

pub fn parse_parent_issue(snapshot: &IssueSnapshot) -> Result<ParentIssue> {
    let sections = parse_sections(&snapshot.body);
    let ordered = sections
        .get("ordered child issues")
        .ok_or_else(|| anyhow!("missing Ordered child Issues section"))?;
    let children = parse_parent_child_refs(ordered)?;
    if children.is_empty() {
        bail!("parent issue contains no child issues");
    }
    Ok(ParentIssue {
        number: snapshot.number,
        title: snapshot.title.clone(),
        body: snapshot.body.clone(),
        state: snapshot.state.clone(),
        labels: snapshot.labels.clone(),
        url: snapshot.url.clone(),
        sections,
        children,
    })
}

pub fn parse_sections(body: &str) -> BTreeMap<String, String> {
    let mut sections = BTreeMap::new();
    let mut current_key: Option<String> = None;
    let mut current_lines: Vec<String> = Vec::new();

    for line in body.lines() {
        if let Some(title) = heading_title(line) {
            if let Some(key) = current_key.take() {
                sections.insert(key, trim_section(&current_lines));
                current_lines.clear();
            }
            current_key = Some(normalize_heading(&title));
        } else if current_key.is_some() {
            current_lines.push(line.to_string());
        }
    }

    if let Some(key) = current_key {
        sections.insert(key, trim_section(&current_lines));
    }

    sections
}

pub fn parse_parent_child_refs(section: &str) -> Result<Vec<ParentChildRef>> {
    let mut children = Vec::new();
    for line in section.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some(rest) = trimmed.strip_prefix("- [") else {
            bail!("invalid parent child checklist line: {trimmed}");
        };
        let checked = rest.starts_with("x]") || rest.starts_with("X]");
        let number = extract_issue_numbers(trimmed)
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("missing child issue number in line: {trimmed}"))?;
        children.push(ParentChildRef {
            number,
            checked,
            raw_line: trimmed.to_string(),
        });
    }
    Ok(children)
}

pub fn parse_source_refs(section: &str) -> Result<Vec<SourceRef>> {
    let mut refs = Vec::new();
    for line in section.lines() {
        let trimmed = strip_list_prefix(line.trim());
        if trimmed.is_empty() {
            continue;
        }
        let kind = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            SourceRefKind::Url {
                url: trimmed.to_string(),
            }
        } else {
            let path = PathBuf::from(trimmed);
            if path.is_absolute() {
                SourceRefKind::Absolute { path }
            } else if trimmed.contains("://") {
                bail!("unsupported source-of-truth URL: {trimmed}");
            } else {
                SourceRefKind::RepoRelative { path }
            }
        };
        refs.push(SourceRef {
            raw: trimmed.to_string(),
            kind,
        });
    }
    Ok(refs)
}

pub fn parse_verification_commands(section: &str) -> Vec<VerificationCommand> {
    let mut commands = Vec::new();
    let mut in_shell_block = false;

    for line in section.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            let lang = trimmed
                .trim_start_matches("```")
                .trim()
                .to_ascii_lowercase();
            if !in_shell_block && matches!(lang.as_str(), "sh" | "bash" | "shell") {
                in_shell_block = true;
                continue;
            }
            if in_shell_block {
                in_shell_block = false;
            }
            continue;
        }

        if in_shell_block {
            if !trimmed.is_empty() && !trimmed.starts_with('#') {
                commands.push(VerificationCommand {
                    command: trimmed.to_string(),
                    source: VerificationSource::FencedShell,
                });
            }
            continue;
        }

        if let Some(command) = trimmed.strip_prefix("cmd:") {
            let command = command.trim();
            if !command.is_empty() {
                commands.push(VerificationCommand {
                    command: command.to_string(),
                    source: VerificationSource::CmdLine,
                });
            }
        }
    }

    commands
}

pub fn parse_dependencies(section: &str) -> Result<Vec<u64>> {
    let trimmed = section.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("none") {
        return Ok(Vec::new());
    }

    let mut cleaned = String::with_capacity(trimmed.len());
    for ch in trimmed.chars() {
        if ch.is_ascii_digit()
            || ch == '#'
            || ch == ','
            || ch == '\n'
            || ch == '\r'
            || ch == '\t'
            || ch == ' '
        {
            cleaned.push(ch);
        } else {
            bail!("invalid dependency token: {trimmed}");
        }
    }

    let mut numbers = Vec::new();
    for token in cleaned.split(|ch: char| ch == ',' || ch.is_whitespace()) {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        let token = token.trim_start_matches('#');
        numbers.push(token.parse::<u64>()?);
    }
    Ok(numbers)
}

pub fn build_child_issue(
    snapshot: &IssueSnapshot,
    sections: BTreeMap<String, String>,
) -> Result<ChildIssue> {
    let background = section_required(&sections, "background")?;
    let goal = section_required(&sections, "goal")?;
    let scope = section_required(&sections, "scope")?;
    let out_of_scope = section_required(&sections, "out of scope")?;
    let source_of_truth_raw = section_required(&sections, "source of truth")?;
    let acceptance_criteria = section_required(&sections, "acceptance criteria")?;
    let verification_raw = section_required(&sections, "verification")?;
    let dependencies_raw = sections
        .get("dependencies")
        .cloned()
        .unwrap_or_else(String::new);
    let implementation_constraints = section_optional(&sections, "implementation constraints");

    let source_of_truth = parse_source_refs(&source_of_truth_raw)?;
    let verification = parse_verification_commands(&verification_raw);
    let dependencies = parse_dependencies(&dependencies_raw)?;
    let target_size = SizeBand::from_text(&section_required(&sections, "target change size")?)
        .ok_or_else(|| anyhow!("invalid target change size"))?;
    let docs_impact = DocsImpact::from_text(&section_required(&sections, "documentation impact")?)
        .ok_or_else(|| anyhow!("invalid documentation impact"))?;
    let suggested_model_profile = section_required(&sections, "suggested model profile")?;
    let suggested_model_override = section_optional(&sections, "suggested model override");
    let estimated_minutes =
        section_required(&sections, "estimated execution time")?.parse::<u32>()?;
    let estimation_basis =
        EstimationBasis::from_text(&section_required(&sections, "estimation basis")?)
            .ok_or_else(|| anyhow!("invalid estimation basis"))?;
    let estimation_confidence =
        Confidence::from_text(&section_required(&sections, "estimation confidence")?)
            .ok_or_else(|| anyhow!("invalid estimation confidence"))?;

    Ok(ChildIssue {
        number: snapshot.number,
        title: snapshot.title.clone(),
        body: snapshot.body.clone(),
        state: snapshot.state.clone(),
        labels: snapshot.labels.clone(),
        url: snapshot.url.clone(),
        sections,
        background,
        goal,
        scope,
        out_of_scope,
        source_of_truth_raw,
        source_of_truth,
        implementation_constraints,
        acceptance_criteria,
        verification_raw,
        verification,
        dependencies_raw,
        dependencies,
        target_size,
        docs_impact,
        suggested_model_profile,
        suggested_model_override,
        estimated_minutes,
        estimation_basis,
        estimation_confidence,
    })
}

pub fn extract_issue_numbers(text: &str) -> Vec<u64> {
    let mut numbers = Vec::new();
    let bytes = text.as_bytes();
    let mut index = 0usize;

    while index < bytes.len() {
        if bytes[index] == b'#' {
            index += 1;
        }
        if index < bytes.len() && bytes[index].is_ascii_digit() {
            let start = index;
            while index < bytes.len() && bytes[index].is_ascii_digit() {
                index += 1;
            }
            if let Ok(number) = text[start..index].parse::<u64>() {
                numbers.push(number);
            }
            continue;
        }
        index += 1;
    }

    numbers
}

pub fn normalize_heading(title: &str) -> String {
    title
        .trim()
        .trim_end_matches(':')
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn heading_title(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if let Some(title) = trimmed.strip_prefix("## ") {
        return Some(title.trim().to_string());
    }
    if let Some(title) = trimmed.strip_prefix("### ") {
        return Some(title.trim().to_string());
    }
    None
}

fn trim_section(lines: &[String]) -> String {
    lines.join("\n").trim().to_string()
}

fn strip_list_prefix(text: &str) -> &str {
    if let Some(rest) = text.strip_prefix("- ") {
        return rest.trim();
    }
    if let Some(rest) = text.strip_prefix("* ") {
        return rest.trim();
    }
    text
}

fn section_required(sections: &BTreeMap<String, String>, key: &str) -> Result<String> {
    sections
        .get(key)
        .cloned()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("missing section: {key}"))
}

fn section_optional(sections: &BTreeMap<String, String>, key: &str) -> Option<String> {
    sections
        .get(key)
        .cloned()
        .filter(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use crate::models::{IssueSnapshot, IssueState, VerificationSource};

    use super::{
        parse_parent_child_refs, parse_sections, parse_source_refs, parse_verification_commands,
    };

    #[test]
    fn headings_treat_h2_and_h3_as_equivalent() {
        let sections = parse_sections(
            "## Background\none\n### Goal\ntwo\n## Verification\n```sh\ncargo test\n```\n",
        );
        assert_eq!(sections.get("background").unwrap(), "one");
        assert_eq!(sections.get("goal").unwrap(), "two");
    }

    #[test]
    fn parent_checklist_parsing_preserves_order() {
        let section = "- [ ] #222 first\n- [x] #223 done\n- [ ] #224 depends on #223";
        let children = parse_parent_child_refs(section).unwrap();
        assert_eq!(
            children.iter().map(|item| item.number).collect::<Vec<_>>(),
            vec![222, 223, 224]
        );
        assert!(children[1].checked);
    }

    #[test]
    fn verification_parses_cmd_and_fenced_shell() {
        let commands = parse_verification_commands(
            "cmd: cargo fmt --check\n\n```sh\ncargo test\n# ignored\ncargo clippy\n```\n",
        );
        assert_eq!(commands.len(), 3);
        assert_eq!(commands[0].source, VerificationSource::CmdLine);
        assert_eq!(commands[1].source, VerificationSource::FencedShell);
    }

    #[test]
    fn source_refs_parse_repo_paths_absolute_and_urls() {
        let refs = parse_source_refs("README.md\n/abs/file.md\nhttps://example.com/doc").unwrap();
        assert_eq!(refs.len(), 3);
    }

    #[test]
    fn issue_snapshot_supports_label_checks() {
        let snapshot = IssueSnapshot {
            number: 1,
            title: "t".to_string(),
            body: String::new(),
            state: IssueState::Open,
            labels: vec!["agent:ready".to_string()],
            url: None,
        };
        assert!(snapshot.has_label("agent:ready"));
    }
}
