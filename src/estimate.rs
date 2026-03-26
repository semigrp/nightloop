use anyhow::{anyhow, bail, Result};

use crate::{
    budget,
    config::{Config, ModelProfile},
    models::{ChildIssue, DocsImpact, IssueEstimate, SizeBand},
    telemetry,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstimateBasis {
    Template,
    Local,
    Hybrid,
}

impl EstimateBasis {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Template => "template",
            Self::Local => "local",
            Self::Hybrid => "hybrid",
        }
    }

    pub fn from_cli_str(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "template" => Ok(Self::Template),
            "local" => Ok(Self::Local),
            "hybrid" => Ok(Self::Hybrid),
            _ => bail!("invalid estimate basis: {value}"),
        }
    }
}

pub fn estimate_child_issue(
    config: &Config,
    child: &ChildIssue,
    basis: EstimateBasis,
) -> Result<IssueEstimate> {
    let profile = choose_model_profile(config, &child.suggested_model_profile)?;
    let model = child
        .suggested_model_override
        .clone()
        .unwrap_or_else(|| profile.model.clone());

    let template_minutes = template_minutes(
        config,
        &child.target_size,
        &child.docs_impact,
        child.dependencies.len(),
        profile.runtime_multiplier,
    );
    let local_stats = telemetry::load_stats(
        &config.telemetry_history_path(),
        &profile.name,
        &child.target_size,
        &child.docs_impact,
    )?;

    let enough_local = local_stats.samples >= config.telemetry.min_samples_for_local;
    let (estimated_minutes, basis_used) = match basis {
        EstimateBasis::Template => (template_minutes, "template".to_string()),
        EstimateBasis::Local if enough_local => (
            local_stats.average_minutes.round() as u32,
            "local".to_string(),
        ),
        EstimateBasis::Local => (template_minutes, "template-fallback".to_string()),
        EstimateBasis::Hybrid if enough_local => (
            weighted_minutes(
                local_stats.average_minutes,
                template_minutes as f32,
                config.telemetry.local_weight,
                config.telemetry.template_weight,
            ),
            "hybrid".to_string(),
        ),
        EstimateBasis::Hybrid => (template_minutes, "template-fallback".to_string()),
    };

    let recommended_hours = recommend_window_hours(
        estimated_minutes,
        config.loop_cfg.fixed_overhead_minutes,
        config.loop_cfg.min_hours,
        config.loop_cfg.max_hours,
    )?;

    Ok(IssueEstimate {
        model_profile: profile.name.clone(),
        model,
        reasoning_effort: profile.reasoning_effort.clone(),
        estimated_minutes,
        recommended_hours,
        basis_requested: basis.as_str().to_string(),
        basis_used,
        local_samples: local_stats.samples,
        notes: Vec::new(),
        ai_estimate: None,
    })
}

fn choose_model_profile<'a>(config: &'a Config, requested: &str) -> Result<&'a ModelProfile> {
    config
        .model_profile(requested)
        .or_else(|| config.default_profile())
        .ok_or_else(|| anyhow!("no model profile configured"))
}

fn template_minutes(
    config: &Config,
    size_band: &SizeBand,
    docs_impact: &DocsImpact,
    dependency_count: usize,
    runtime_multiplier: f32,
) -> u32 {
    let base = match size_band {
        SizeBand::Xs => config.estimation.template_minutes_xs,
        SizeBand::S => config.estimation.template_minutes_s,
        SizeBand::M => config.estimation.template_minutes_m,
        SizeBand::L => config.estimation.template_minutes_l,
    };

    let docs_penalty = match docs_impact {
        DocsImpact::None => 0,
        DocsImpact::Readme => config.estimation.docs_penalty_readme,
        DocsImpact::UserFacingDocs => config.estimation.docs_penalty_user_facing,
        DocsImpact::ArchitectureDocs => config.estimation.docs_penalty_architecture,
    };

    let dependency_penalty = dependency_count as u32 * config.estimation.dependency_penalty_minutes;
    (((base + docs_penalty + dependency_penalty) as f32) * runtime_multiplier).round() as u32
}

fn weighted_minutes(local: f32, template: f32, local_weight: f32, template_weight: f32) -> u32 {
    let total = local_weight + template_weight;
    if total <= f32::EPSILON {
        return template.round() as u32;
    }
    (((local * local_weight) + (template * template_weight)) / total).round() as u32
}

fn recommend_window_hours(
    estimated_minutes: u32,
    fixed_overhead_minutes: u32,
    min_hours: u32,
    max_hours: u32,
) -> Result<u32> {
    let total_minutes = estimated_minutes + fixed_overhead_minutes;
    for hours in min_hours..=max_hours {
        if budget::available_minutes(hours, fixed_overhead_minutes, min_hours, max_hours)?
            >= estimated_minutes
        {
            return Ok(hours);
        }
    }
    Ok(((total_minutes + 59) / 60).min(max_hours).max(min_hours))
}

#[cfg(test)]
mod tests {
    use std::{env, fs};

    use crate::config::Config;

    use super::{estimate_child_issue, EstimateBasis};

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
min_samples_for_local = 2
local_weight = 0.65
template_weight = 0.35
"#,
                root.display()
            ),
        )
        .unwrap();
        Config::load(&path).unwrap()
    }

    fn child() -> crate::models::ChildIssue {
        use crate::models::{Confidence, DocsImpact, EstimationBasis, IssueState, SizeBand};
        crate::models::ChildIssue {
            number: 1,
            title: "title".to_string(),
            body: "body".to_string(),
            state: IssueState::Open,
            labels: vec![],
            url: None,
            sections: Default::default(),
            background: String::new(),
            goal: String::new(),
            scope: "docs-only".to_string(),
            out_of_scope: String::new(),
            source_of_truth_raw: String::new(),
            source_of_truth: vec![],
            implementation_constraints: None,
            acceptance_criteria: String::new(),
            verification_raw: String::new(),
            verification: vec![],
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
    fn template_and_local_estimates_behave_as_expected() {
        let root = env::temp_dir().join(format!("nightloop-estimate-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let config = config(&root);
        let report = estimate_child_issue(&config, &child(), EstimateBasis::Template).unwrap();
        assert_eq!(report.estimated_minutes, 85);
        let hybrid = estimate_child_issue(&config, &child(), EstimateBasis::Hybrid).unwrap();
        assert_eq!(hybrid.basis_used, "template-fallback");
    }

    #[test]
    fn invalid_cli_basis_is_rejected() {
        assert!(EstimateBasis::from_cli_str("ai").is_err());
    }
}
