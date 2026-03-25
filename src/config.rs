use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub github: GitHub,
    pub agent: Agent,
    #[serde(rename = "loop")]
    pub loop_cfg: LoopConfig,
    pub diff: DiffConfig,
    pub labels: Labels,
    pub docs: DocsConfig,
    pub estimation: EstimationConfig,
    pub telemetry: TelemetryConfig,
}

#[derive(Debug, Deserialize)]
pub struct GitHub {
    pub owner: String,
    pub repo: String,
    pub base_branch: String,
    #[serde(default)]
    pub request_copilot_review: bool,
    #[serde(default = "default_copilot_reviewer")]
    pub copilot_reviewer: String,
}

#[derive(Debug, Deserialize)]
pub struct Agent {
    pub command: String,
    pub plan_command: String,
    pub working_directory: String,
    pub default_model: String,
    pub default_reasoning_effort: String,
    pub model_profiles: Vec<ModelProfile>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ModelProfile {
    pub name: String,
    pub model: String,
    pub reasoning_effort: String,
    pub intended_for: String,
    pub max_size: String,
    pub runtime_multiplier: f32,
}

#[derive(Debug, Deserialize)]
pub struct LoopConfig {
    pub default_hours: u32,
    pub min_hours: u32,
    pub max_hours: u32,
    pub fallback_cycle_minutes: u32,
    pub fixed_overhead_minutes: u32,
    pub stop_on_failure: bool,
    pub one_branch_per_child: bool,
    pub one_pr_per_child: bool,
}

#[derive(Debug, Deserialize)]
pub struct DiffConfig {
    pub min_lines: u32,
    pub max_lines: u32,
    pub allow_doc_only_below_min: bool,
}

#[derive(Debug, Deserialize)]
pub struct Labels {
    pub campaign: String,
    pub night_run: String,
    pub ready: String,
    pub running: String,
    pub review: String,
    pub blocked: String,
    pub done: String,
}

#[derive(Debug, Deserialize)]
pub struct DocsConfig {
    pub required_paths: Vec<PathBuf>,
}

#[derive(Debug, Deserialize)]
pub struct EstimationConfig {
    pub default_basis: String,
    pub allow_ai_assist: bool,
    pub template_minutes_xs: u32,
    pub template_minutes_s: u32,
    pub template_minutes_m: u32,
    pub template_minutes_l: u32,
    pub dependency_penalty_minutes: u32,
    pub docs_penalty_readme: u32,
    pub docs_penalty_user_facing: u32,
    pub docs_penalty_architecture: u32,
}

#[derive(Debug, Deserialize)]
pub struct TelemetryConfig {
    pub history_path: PathBuf,
    pub min_samples_for_local: usize,
    pub local_weight: f32,
    pub template_weight: f32,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read config from {}", path.display()))?;
        let cfg = toml::from_str::<Self>(&raw)
            .with_context(|| format!("failed to parse TOML config from {}", path.display()))?;
        Ok(cfg)
    }

    pub fn model_profile(&self, name: &str) -> Option<&ModelProfile> {
        self.agent
            .model_profiles
            .iter()
            .find(|profile| profile.name.eq_ignore_ascii_case(name))
    }

    pub fn default_profile(&self) -> Option<&ModelProfile> {
        self.model_profile("balanced")
            .or_else(|| self.agent.model_profiles.first())
    }

    pub fn repo_slug(&self) -> String {
        format!("{}/{}", self.github.owner, self.github.repo)
    }

    pub fn working_directory(&self) -> PathBuf {
        PathBuf::from(&self.agent.working_directory)
    }
}

fn default_copilot_reviewer() -> String {
    "github-copilot[bot]".to_string()
}
