use std::{env, fs};

use anyhow::{anyhow, bail, Context, Result};

use crate::{
    agent_exec, budget,
    config::{Config, ModelProfile},
    models::{AiEstimate, ChildIssue, DocsImpact, IssueEstimate, SizeBand},
    telemetry,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstimateBasis {
    Template,
    Local,
    Hybrid,
    Ai,
}

impl EstimateBasis {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Template => "template",
            Self::Local => "local",
            Self::Hybrid => "hybrid",
            Self::Ai => "ai",
        }
    }

    pub fn from_cli_str(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "template" => Ok(Self::Template),
            "local" => Ok(Self::Local),
            "hybrid" => Ok(Self::Hybrid),
            "ai" => Ok(Self::Ai),
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
        &config.telemetry.history_path,
        &profile.name,
        &child.target_size,
        &child.docs_impact,
    )?;

    let enough_local = local_stats.samples >= config.telemetry.min_samples_for_local;
    let (estimated_minutes, baseline_basis_used) = match basis {
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
        EstimateBasis::Ai if enough_local => (
            weighted_minutes(
                local_stats.average_minutes,
                template_minutes as f32,
                config.telemetry.local_weight,
                config.telemetry.template_weight,
            ),
            "hybrid".to_string(),
        ),
        EstimateBasis::Ai => (template_minutes, "template".to_string()),
    };

    let mut notes = Vec::new();
    let ai_estimate = if basis == EstimateBasis::Ai {
        if config.estimation.allow_ai_assist {
            match request_ai_estimate(config, child) {
                Ok(ai) => Some(ai),
                Err(err) => {
                    notes.push(format!("ai_fallback={}", err));
                    None
                }
            }
        } else {
            notes.push("ai_fallback=allow_ai_assist_disabled".to_string());
            None
        }
    } else {
        None
    };

    let basis_used = if basis == EstimateBasis::Ai {
        if ai_estimate.is_some() {
            format!("ai+{baseline_basis_used}")
        } else {
            format!("{baseline_basis_used}-ai-fallback")
        }
    } else {
        baseline_basis_used
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
        notes,
        ai_estimate,
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

fn request_ai_estimate(config: &Config, child: &ChildIssue) -> Result<AiEstimate> {
    let template_path = config.working_directory().join("prompts/estimate_issue.md");
    let template = fs::read_to_string(&template_path)
        .with_context(|| format!("failed to read {}", template_path.display()))?;
    let prompt = format!(
        "{template}\n\nIssue title: {}\nTarget change size: {}\nDocumentation impact: {}\nDependencies: {}\n\nIssue body:\n{}\n",
        child.title,
        child.target_size.as_str(),
        child.docs_impact.as_str(),
        if child.dependencies.is_empty() {
            "none".to_string()
        } else {
            child
                .dependencies
                .iter()
                .map(|value| value.to_string())
                .collect::<Vec<_>>()
                .join(",")
        },
        child.body
    );

    let temp_dir = env::temp_dir().join(format!(
        "nightloop-estimate-{}-{}",
        std::process::id(),
        child.number
    ));
    fs::create_dir_all(&temp_dir)?;
    let prompt_path = temp_dir.join("estimate-prompt.md");
    fs::write(&prompt_path, prompt)?;

    let result = agent_exec::run_shell_command(
        &config.agent.plan_command,
        &config.working_directory(),
        &[
            (
                "NIGHTLOOP_PROMPT_FILE".to_string(),
                prompt_path.display().to_string(),
            ),
            (
                "NIGHTLOOP_CHILD_ISSUE".to_string(),
                child.number.to_string(),
            ),
            ("NIGHTLOOP_CHILD_TITLE".to_string(), child.title.clone()),
        ],
        None,
    )?;
    if !result.success() {
        bail!("ai_command_failed");
    }

    parse_ai_estimate(&result.stdout)
}

fn parse_ai_estimate(stdout: &str) -> Result<AiEstimate> {
    #[derive(serde::Deserialize)]
    struct RawAiEstimate {
        model_profile: String,
        estimated_minutes: u32,
        confidence: String,
        notes: String,
    }

    let json = extract_json(stdout).ok_or_else(|| anyhow!("ai_output_missing_json"))?;
    let parsed = serde_json::from_str::<RawAiEstimate>(&json).context("ai_output_invalid_json")?;
    let confidence = crate::models::Confidence::from_text(&parsed.confidence)
        .ok_or_else(|| anyhow!("ai_output_invalid_confidence"))?;
    Ok(AiEstimate {
        model_profile: parsed.model_profile,
        estimated_minutes: parsed.estimated_minutes,
        confidence,
        notes: parsed.notes,
    })
}

fn extract_json(text: &str) -> Option<String> {
    if text.trim_start().starts_with('{') {
        return Some(text.trim().to_string());
    }
    if let Some(start) = text.find("```json") {
        let rest = &text[start + "```json".len()..];
        let end = rest.find("```")?;
        return Some(rest[..end].trim().to_string());
    }
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    Some(text[start..=end].trim().to_string())
}

#[cfg(test)]
mod tests {
    use std::{env, fs};

    use chrono::Utc;

    use crate::{
        config::Config,
        models::{
            ChildIssue, Confidence, DocsImpact, EstimationBasis, IssueState, RunRecord, SizeBand,
        },
        telemetry,
    };

    use super::{estimate_child_issue, parse_ai_estimate, EstimateBasis};

    fn config(root: &std::path::Path) -> Config {
        let config_path = root.join("config.toml");
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
required_paths = []

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
min_samples_for_local = 2
local_weight = 0.65
template_weight = 0.35
"#,
                root.display(),
                root.join("history.jsonl").display()
            ),
        )
        .unwrap();
        Config::load(&config_path).unwrap()
    }

    fn child() -> ChildIssue {
        ChildIssue {
            number: 1,
            title: "title".to_string(),
            body: "body".to_string(),
            state: IssueState::Open,
            labels: Vec::new(),
            url: None,
            sections: Default::default(),
            background: String::new(),
            goal: String::new(),
            scope: "normal".to_string(),
            out_of_scope: String::new(),
            source_of_truth_raw: String::new(),
            source_of_truth: Vec::new(),
            implementation_constraints: None,
            acceptance_criteria: String::new(),
            verification_raw: String::new(),
            verification: Vec::new(),
            dependencies_raw: "2".to_string(),
            dependencies: vec![2],
            target_size: SizeBand::M,
            docs_impact: DocsImpact::Readme,
            suggested_model_profile: "balanced".to_string(),
            suggested_model_override: None,
            estimated_minutes: 80,
            estimation_basis: EstimationBasis::Template,
            estimation_confidence: Confidence::Medium,
        }
    }

    #[test]
    fn template_and_local_estimates_behave_as_expected() {
        let root = env::temp_dir().join(format!("nightloop-estimate-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(root.join("prompts")).unwrap();
        fs::write(root.join("prompts/estimate_issue.md"), "Return JSON only.").unwrap();

        let config = config(&root);
        let report = estimate_child_issue(&config, &child(), EstimateBasis::Template).unwrap();
        assert_eq!(report.estimated_minutes, 90);

        telemetry::append_run_record(
            &config.telemetry.history_path,
            &RunRecord {
                run_id: "1".to_string(),
                parent_issue: 10,
                issue_number: 11,
                issue_title: "a".to_string(),
                model_profile: "balanced".to_string(),
                model: "gpt-5.4".to_string(),
                reasoning_effort: "medium".to_string(),
                target_size: SizeBand::M,
                docs_impact: DocsImpact::Readme,
                estimated_minutes: 95,
                actual_minutes: 100,
                changed_lines: 200,
                files_touched: 2,
                success: true,
                status: "success".to_string(),
                copilot_review: None,
                branch: "b".to_string(),
                pr_base: "main".to_string(),
                pr_url: None,
                recorded_at: Utc::now(),
            },
        )
        .unwrap();
        telemetry::append_run_record(
            &config.telemetry.history_path,
            &RunRecord {
                actual_minutes: 80,
                ..RunRecord {
                    run_id: "2".to_string(),
                    parent_issue: 10,
                    issue_number: 12,
                    issue_title: "b".to_string(),
                    model_profile: "balanced".to_string(),
                    model: "gpt-5.4".to_string(),
                    reasoning_effort: "medium".to_string(),
                    target_size: SizeBand::M,
                    docs_impact: DocsImpact::Readme,
                    estimated_minutes: 95,
                    actual_minutes: 80,
                    changed_lines: 200,
                    files_touched: 2,
                    success: true,
                    status: "success".to_string(),
                    copilot_review: None,
                    branch: "c".to_string(),
                    pr_base: "main".to_string(),
                    pr_url: None,
                    recorded_at: Utc::now(),
                }
            },
        )
        .unwrap();
        let hybrid = estimate_child_issue(&config, &child(), EstimateBasis::Hybrid).unwrap();
        assert!(hybrid.estimated_minutes < 95);
        let ai = estimate_child_issue(&config, &child(), EstimateBasis::Ai).unwrap();
        assert!(ai.ai_estimate.is_some());
    }

    #[test]
    fn ai_output_parser_accepts_fenced_json() {
        let ai = parse_ai_estimate(
            "```json\n{\"model_profile\":\"balanced\",\"estimated_minutes\":65,\"confidence\":\"medium\",\"notes\":\"ok\"}\n```",
        )
        .unwrap();
        assert_eq!(ai.estimated_minutes, 65);
    }
}
