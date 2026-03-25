use std::{fs, path::Path};

use anyhow::{anyhow, Context, Result};
use regex::Regex;

use crate::{
    config::{Config, ModelProfile},
    models::{DocsImpact, SizeBand},
    telemetry,
};

#[derive(Debug, Clone, Copy)]
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
}

#[derive(Debug)]
pub struct EstimateReport {
    pub model_profile: String,
    pub model: String,
    pub reasoning_effort: String,
    pub estimated_minutes: u32,
    pub recommended_hours: u32,
    pub basis_requested: String,
    pub basis_used: String,
    pub local_samples: usize,
    pub notes: Option<String>,
}

pub fn estimate_issue(config: &Config, path: &Path, basis: EstimateBasis) -> Result<EstimateReport> {
    let body = fs::read_to_string(path)
        .with_context(|| format!("failed to read issue markdown from {}", path.display()))?;

    let size_band = parse_size_band(&body).unwrap_or(SizeBand::M);
    let docs_impact = parse_docs_impact(&body).unwrap_or(DocsImpact::None);
    let dependency_count = parse_dependency_count(&body);
    let requested_profile = extract_section(&body, "Suggested model profile").map(|s| s.trim().to_ascii_lowercase());
    let requested_model_override = extract_section(&body, "Suggested model override").map(|s| s.trim().to_string());

    let profile = choose_model_profile(config, requested_profile.as_deref(), &size_band, &docs_impact)?;
    let model = requested_model_override.unwrap_or_else(|| profile.model.clone());

    let template_minutes = template_minutes(config, &size_band, &docs_impact, dependency_count, profile.runtime_multiplier);
    let local_stats = telemetry::load_stats(
        &config.telemetry.history_path,
        &profile.name,
        &size_band,
        &docs_impact,
    )?;

    let has_enough_local = local_stats.samples >= config.telemetry.min_samples_for_local;
    let basis_used = match basis {
        EstimateBasis::Template => "template",
        EstimateBasis::Local if has_enough_local => "local",
        EstimateBasis::Local => "template-fallback",
        EstimateBasis::Hybrid if has_enough_local => "hybrid",
        EstimateBasis::Hybrid => "template-fallback",
        EstimateBasis::Ai => {
            if has_enough_local {
                "hybrid-ai-fallback"
            } else {
                "template-ai-fallback"
            }
        }
    };

    let estimated_minutes = match basis {
        EstimateBasis::Template => template_minutes,
        EstimateBasis::Local if has_enough_local => local_stats.average_minutes.round() as u32,
        EstimateBasis::Local => template_minutes,
        EstimateBasis::Hybrid if has_enough_local => weighted_minutes(
            local_stats.average_minutes,
            template_minutes as f32,
            config.telemetry.local_weight,
            config.telemetry.template_weight,
        ),
        EstimateBasis::Hybrid => template_minutes,
        EstimateBasis::Ai => {
            if has_enough_local {
                weighted_minutes(
                    local_stats.average_minutes,
                    template_minutes as f32,
                    config.telemetry.local_weight,
                    config.telemetry.template_weight,
                )
            } else {
                template_minutes
            }
        }
    };

    let recommended_hours = recommend_window_hours(
        estimated_minutes,
        config.loop_cfg.fixed_overhead_minutes,
        config.loop_cfg.min_hours,
        config.loop_cfg.max_hours,
    );

    let notes = match basis {
        EstimateBasis::Ai if config.estimation.allow_ai_assist => Some(
            "ai basis requested: use prompts/estimate_issue.md with your configured agent command and compare against the local/template estimate before finalizing the issue".to_string(),
        ),
        EstimateBasis::Ai => Some(
            "ai basis requested but allow_ai_assist=false in config; returning local/template fallback".to_string(),
        ),
        _ => None,
    };

    Ok(EstimateReport {
        model_profile: profile.name.clone(),
        model,
        reasoning_effort: profile.reasoning_effort.clone(),
        estimated_minutes,
        recommended_hours,
        basis_requested: basis.as_str().to_string(),
        basis_used: basis_used.to_string(),
        local_samples: local_stats.samples,
        notes,
    })
}

fn choose_model_profile<'a>(
    config: &'a Config,
    requested: Option<&str>,
    size_band: &SizeBand,
    docs_impact: &DocsImpact,
) -> Result<&'a ModelProfile> {
    if let Some(name) = requested {
        if let Some(profile) = config.model_profile(name) {
            return Ok(profile);
        }
    }

    let inferred = match (size_band, docs_impact) {
        (SizeBand::Xs, DocsImpact::None | DocsImpact::Readme) => "fast",
        (SizeBand::S, DocsImpact::None | DocsImpact::Readme) => "fast",
        (SizeBand::L, _) => "deep",
        (_, DocsImpact::Architecture) => "deep",
        _ => "balanced",
    };

    config
        .model_profile(inferred)
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
        DocsImpact::UserFacing => config.estimation.docs_penalty_user_facing,
        DocsImpact::Architecture => config.estimation.docs_penalty_architecture,
    };

    let dependency_penalty = dependency_count as u32 * config.estimation.dependency_penalty_minutes;
    (((base + docs_penalty + dependency_penalty) as f32) * runtime_multiplier).round() as u32
}

fn weighted_minutes(local: f32, template: f32, local_weight: f32, template_weight: f32) -> u32 {
    let total_weight = local_weight + template_weight;
    if total_weight <= f32::EPSILON {
        return template.round() as u32;
    }
    (((local * local_weight) + (template * template_weight)) / total_weight).round() as u32
}

fn recommend_window_hours(estimated_minutes: u32, fixed_overhead_minutes: u32, min_hours: u32, max_hours: u32) -> u32 {
    let total = estimated_minutes + fixed_overhead_minutes;
    let hours = ((total + 59) / 60).max(min_hours);
    hours.min(max_hours)
}

fn parse_size_band(body: &str) -> Option<SizeBand> {
    let text = extract_section(body, "Target change size")?.to_ascii_lowercase();
    if text.contains("xs") {
        Some(SizeBand::Xs)
    } else if text.contains(" s ") || text.starts_with('s') {
        Some(SizeBand::S)
    } else if text.contains(" m ") || text.starts_with('m') {
        Some(SizeBand::M)
    } else if text.contains(" l ") || text.starts_with('l') {
        Some(SizeBand::L)
    } else {
        None
    }
}

fn parse_docs_impact(body: &str) -> Option<DocsImpact> {
    let text = extract_section(body, "Documentation impact")?.to_ascii_lowercase();
    if text.contains("architecture") {
        Some(DocsImpact::Architecture)
    } else if text.contains("user-facing") || text.contains("user facing") {
        Some(DocsImpact::UserFacing)
    } else if text.contains("readme") {
        Some(DocsImpact::Readme)
    } else if text.contains("none") {
        Some(DocsImpact::None)
    } else {
        None
    }
}

fn parse_dependency_count(body: &str) -> usize {
    let Some(section) = extract_section(body, "Dependencies") else {
        return 0;
    };
    let re = Regex::new(r"#?(\d+)").expect("valid regex");
    re.captures_iter(&section).count()
}

fn extract_section(body: &str, title: &str) -> Option<String> {
    let heading = format!("## {title}");
    let start = body.find(&heading)? + heading.len();
    let rest = &body[start..];
    let end = rest.find("\n## ").unwrap_or(rest.len());
    Some(rest[..end].trim().to_string())
}
