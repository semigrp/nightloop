use std::fs;

use anyhow::{Context, Result};

use crate::{
    config::Config,
    control_assets,
    intent_bundle::IntentBundle,
    models::{IssueEstimate, ParentIssue},
};

pub fn build_plan_prompt(
    config: &Config,
    parent: &ParentIssue,
    bundle: &IntentBundle,
    estimate: &IssueEstimate,
) -> Result<String> {
    let manifest = control_assets::manifest();
    let template_path = config.resolve_control_path(&manifest.plan_prompt.path);
    let template = fs::read_to_string(&template_path)
        .with_context(|| format!("failed to read {}", template_path.display()))?;

    Ok(format!(
        "{template}\n\n{}\n",
        render_bundle_context(parent, bundle, estimate, "start", None)
    ))
}

pub fn build_implementation_prompt(
    parent: &ParentIssue,
    bundle: &IntentBundle,
    estimate: &IssueEstimate,
    workflow: &str,
    plan_output: Option<&str>,
) -> String {
    let mut prompt = String::new();
    prompt.push_str("Implement only the declared child issue scope.\n");
    prompt.push_str("Use the resolved source-of-truth excerpts as the contract anchor.\n");
    prompt.push_str("Run the declared verification commands before finishing.\n\n");
    prompt.push_str(&render_bundle_context(
        parent,
        bundle,
        estimate,
        workflow,
        plan_output,
    ));
    prompt
}

fn render_bundle_context(
    parent: &ParentIssue,
    bundle: &IntentBundle,
    estimate: &IssueEstimate,
    workflow: &str,
    plan_output: Option<&str>,
) -> String {
    let child = &bundle.child;
    let mut prompt = format!(
        "Parent issue: #{} {}\nChild issue: #{} {}\nWorkflow: {}\nModel profile: {}\nEstimated minutes: {}\n\n## Child Contract\n### Background\n{}\n\n### Goal\n{}\n\n### Scope\n{}\n\n### Out of scope\n{}\n\n### Acceptance criteria\n{}\n\n### Documentation impact\n{}\n\n### Target change size\n{}\n",
        parent.number,
        parent.title,
        child.number,
        child.title,
        workflow,
        estimate.model_profile,
        estimate.estimated_minutes,
        child.background,
        child.goal,
        child.scope,
        child.out_of_scope,
        child.acceptance_criteria,
        bundle.docs_impact.as_str(),
        bundle.target_size.as_str(),
    );

    if let Some(constraints) = &child.implementation_constraints {
        prompt.push_str(&format!(
            "\n### Implementation constraints\n{constraints}\n"
        ));
    }

    prompt.push_str("\n## Verification\n");
    for command in &bundle.verification {
        prompt.push_str(&format!("- {}\n", command.command));
    }

    prompt.push_str("\n## Source of truth\n");
    if bundle.resolved_sources.is_empty() {
        prompt.push_str("- none\n");
    } else {
        for source in &bundle.resolved_sources {
            prompt.push_str(&format!(
                "### {}\n- sha256: {}\n- truncated: {}\n````text\n{}\n````\n",
                source.source_ref.resolved_path.display(),
                source.sha256,
                source.truncated,
                source.contents,
            ));
        }
    }

    if let Some(plan_output) = plan_output {
        prompt.push_str("\n## Planner output\n");
        prompt.push_str("````markdown\n");
        prompt.push_str(plan_output);
        if !plan_output.ends_with('\n') {
            prompt.push('\n');
        }
        prompt.push_str("````\n");
    }

    prompt
}

#[cfg(test)]
mod tests {
    use crate::{
        intent_bundle::{
            EstimateMetadata, IntentBundle, ResolvedSource, ResolvedSourceRef, SourceLocation,
        },
        models::{
            ChildIssue, Confidence, DocsImpact, EstimationBasis, IssueEstimate, IssueState,
            ParentIssue, SizeBand, VerificationCommand, VerificationSource,
        },
    };

    use super::build_implementation_prompt;

    #[test]
    fn implementation_prompt_includes_resolved_sources_and_plan() {
        let prompt = build_implementation_prompt(
            &ParentIssue {
                number: 1,
                title: "Parent".to_string(),
                body: String::new(),
                state: IssueState::Open,
                labels: vec![],
                url: None,
                sections: Default::default(),
                children: vec![],
            },
            &IntentBundle {
                child: ChildIssue {
                    number: 2,
                    title: "Child".to_string(),
                    body: String::new(),
                    state: IssueState::Open,
                    labels: vec![],
                    url: None,
                    sections: Default::default(),
                    background: "background".to_string(),
                    goal: "goal".to_string(),
                    scope: "scope".to_string(),
                    out_of_scope: "out".to_string(),
                    source_of_truth_raw: "README.md".to_string(),
                    source_of_truth: vec![],
                    implementation_constraints: Some("constraint".to_string()),
                    acceptance_criteria: "accept".to_string(),
                    verification_raw: "cmd: cargo test".to_string(),
                    verification: vec![VerificationCommand {
                        command: "cargo test".to_string(),
                        source: VerificationSource::CmdLine,
                    }],
                    dependencies_raw: "none".to_string(),
                    dependencies: vec![],
                    target_size: SizeBand::M,
                    docs_impact: DocsImpact::None,
                    suggested_model_profile: "balanced".to_string(),
                    suggested_model_override: None,
                    estimated_minutes: 30,
                    estimation_basis: EstimationBasis::Template,
                    estimation_confidence: Confidence::Medium,
                },
                resolved_sources: vec![ResolvedSource {
                    source_ref: ResolvedSourceRef {
                        raw: "README.md".to_string(),
                        resolved_path: std::path::PathBuf::from("/tmp/README.md"),
                        location: SourceLocation::RepoRelative,
                    },
                    contents: "source body".to_string(),
                    sha256: "abc".to_string(),
                    truncated: false,
                }],
                verification: vec![VerificationCommand {
                    command: "cargo test".to_string(),
                    source: VerificationSource::CmdLine,
                }],
                docs_impact: DocsImpact::None,
                target_size: SizeBand::M,
                estimate_metadata: EstimateMetadata {
                    suggested_model_profile: "balanced".to_string(),
                    suggested_model_override: None,
                    estimated_minutes: 30,
                    estimation_basis: EstimationBasis::Template,
                    estimation_confidence: Confidence::Medium,
                },
            },
            &IssueEstimate {
                model_profile: "balanced".to_string(),
                model: "gpt-5.4".to_string(),
                reasoning_effort: "medium".to_string(),
                estimated_minutes: 40,
                recommended_hours: 2,
                basis_requested: "template".to_string(),
                basis_used: "template".to_string(),
                local_samples: 0,
                notes: vec![],
                ai_estimate: None,
            },
            "start",
            Some("# plan"),
        );

        assert!(prompt.contains("source body"));
        assert!(prompt.contains("sha256: abc"));
        assert!(prompt.contains("## Planner output"));
        assert!(prompt.contains("constraint"));
    }
}
