use std::fs;
use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    config::Config,
    models::{
        ChildIssue, Confidence, DocsImpact, EstimationBasis, SizeBand, SourceRef, SourceRefKind,
        VerificationCommand,
    },
};

pub const MAX_SOURCE_BYTES_PER_FILE: usize = 32 * 1024;
pub const MAX_SOURCE_BYTES_TOTAL: usize = 128 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceLocation {
    RepoRelative,
    Absolute,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedSourceRef {
    pub raw: String,
    pub resolved_path: PathBuf,
    pub location: SourceLocation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedSource {
    pub source_ref: ResolvedSourceRef,
    pub contents: String,
    pub sha256: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EstimateMetadata {
    pub suggested_model_profile: String,
    pub suggested_model_override: Option<String>,
    pub estimated_minutes: u32,
    pub estimation_basis: EstimationBasis,
    pub estimation_confidence: Confidence,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentBundle {
    pub child: ChildIssue,
    pub resolved_sources: Vec<ResolvedSource>,
    pub verification: Vec<VerificationCommand>,
    pub docs_impact: DocsImpact,
    pub target_size: SizeBand,
    pub estimate_metadata: EstimateMetadata,
}

pub fn build_intent_bundle(config: &Config, child: &ChildIssue) -> Result<IntentBundle> {
    let mut remaining_total = MAX_SOURCE_BYTES_TOTAL;
    let mut resolved_sources = Vec::new();

    for source_ref in &child.source_of_truth {
        let resolved_ref = resolve_source_ref(config, source_ref)?;
        let raw = fs::read(&resolved_ref.resolved_path).with_context(|| {
            format!(
                "failed to read source-of-truth file {}",
                resolved_ref.resolved_path.display()
            )
        })?;
        let sha256 = format!("{:x}", Sha256::digest(&raw));

        let max_for_source = remaining_total.min(MAX_SOURCE_BYTES_PER_FILE);
        let bytes_to_take = raw.len().min(max_for_source);
        let truncated = raw.len() > bytes_to_take;
        let contents = String::from_utf8_lossy(&raw[..bytes_to_take]).into_owned();
        remaining_total = remaining_total.saturating_sub(bytes_to_take);

        resolved_sources.push(ResolvedSource {
            source_ref: resolved_ref,
            contents,
            sha256,
            truncated,
        });
    }

    Ok(IntentBundle {
        child: child.clone(),
        resolved_sources,
        verification: child.verification.clone(),
        docs_impact: child.docs_impact.clone(),
        target_size: child.target_size.clone(),
        estimate_metadata: EstimateMetadata {
            suggested_model_profile: child.suggested_model_profile.clone(),
            suggested_model_override: child.suggested_model_override.clone(),
            estimated_minutes: child.estimated_minutes,
            estimation_basis: child.estimation_basis.clone(),
            estimation_confidence: child.estimation_confidence.clone(),
        },
    })
}

fn resolve_source_ref(config: &Config, source_ref: &SourceRef) -> Result<ResolvedSourceRef> {
    let (resolved_path, location) = match &source_ref.kind {
        SourceRefKind::RepoRelative { path } => (
            config.resolve_target_path(path),
            SourceLocation::RepoRelative,
        ),
        SourceRefKind::Absolute { path } => {
            (config.resolve_target_path(path), SourceLocation::Absolute)
        }
        SourceRefKind::Url { url } => bail!("unsupported source-of-truth URL: {url}"),
    };

    if !resolved_path.exists() {
        return Err(anyhow!(
            "missing source-of-truth path {}",
            resolved_path.display()
        ));
    }

    Ok(ResolvedSourceRef {
        raw: source_ref.raw.clone(),
        resolved_path,
        location,
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::{env, fs};

    use crate::{
        config::Config,
        models::{ChildIssue, Confidence, DocsImpact, EstimationBasis, IssueState, SizeBand},
    };

    use super::{build_intent_bundle, MAX_SOURCE_BYTES_PER_FILE};

    fn config(root: &std::path::Path) -> Config {
        let path = root.join("nightloop.toml");
        fs::write(
            &path,
            format!(
                r#"[github]
owner = "o"
repo = "r"
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
                root.display()
            ),
        )
        .unwrap();
        Config::load(&path).unwrap()
    }

    fn child(source_path: &str) -> ChildIssue {
        ChildIssue {
            number: 1,
            title: "title".to_string(),
            body: "body".to_string(),
            state: IssueState::Open,
            labels: vec![],
            url: None,
            sections: Default::default(),
            background: "background".to_string(),
            goal: "goal".to_string(),
            scope: "scope".to_string(),
            out_of_scope: "out".to_string(),
            source_of_truth_raw: source_path.to_string(),
            source_of_truth: vec![crate::models::SourceRef {
                raw: source_path.to_string(),
                kind: crate::models::SourceRefKind::RepoRelative {
                    path: PathBuf::from(source_path),
                },
            }],
            implementation_constraints: None,
            acceptance_criteria: "acceptance".to_string(),
            verification_raw: "cmd: cargo test".to_string(),
            verification: vec![crate::models::VerificationCommand {
                command: "cargo test".to_string(),
                source: crate::models::VerificationSource::CmdLine,
            }],
            dependencies_raw: "none".to_string(),
            dependencies: vec![],
            target_size: SizeBand::M,
            docs_impact: DocsImpact::Readme,
            suggested_model_profile: "balanced".to_string(),
            suggested_model_override: None,
            estimated_minutes: 30,
            estimation_basis: EstimationBasis::Template,
            estimation_confidence: Confidence::Medium,
        }
    }

    #[test]
    fn resolves_sources_and_tracks_truncation() {
        let root = env::temp_dir().join(format!("nightloop-intent-bundle-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let content = "a".repeat(MAX_SOURCE_BYTES_PER_FILE + 1024);
        fs::write(root.join("README.md"), &content).unwrap();
        fs::write(root.join("AGENTS.md"), "agents").unwrap();

        let config = config(&root);
        let bundle = build_intent_bundle(&config, &child("README.md")).unwrap();
        assert_eq!(bundle.resolved_sources.len(), 1);
        assert!(bundle.resolved_sources[0].truncated);
        assert_eq!(
            bundle.resolved_sources[0].contents.len(),
            MAX_SOURCE_BYTES_PER_FILE
        );
        assert_eq!(bundle.resolved_sources[0].sha256.len(), 64);
        assert_eq!(bundle.verification.len(), 1);
    }
}
