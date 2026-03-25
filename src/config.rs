use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
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
    #[serde(skip)]
    control_root: PathBuf,
    #[serde(skip)]
    target_repo_root: PathBuf,
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
    #[serde(default = "default_run_root")]
    pub run_root: PathBuf,
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
        let control_root = path.parent().unwrap_or_else(|| Path::new("."));
        Self::load_with_control_root(path, control_root)
    }

    pub fn load_with_control_root(path: &Path, control_root: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read config from {}", path.display()))?;
        let mut cfg = toml::from_str::<Self>(&raw)
            .with_context(|| format!("failed to parse TOML config from {}", path.display()))?;
        cfg.control_root = normalize_path(&control_root);
        cfg.target_repo_root =
            normalize_path(&if Path::new(&cfg.agent.working_directory).is_absolute() {
                PathBuf::from(&cfg.agent.working_directory)
            } else {
                cfg.control_root.join(&cfg.agent.working_directory)
            });
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

    pub fn control_root(&self) -> &Path {
        &self.control_root
    }

    pub fn target_repo_root(&self) -> &Path {
        &self.target_repo_root
    }

    pub fn working_directory(&self) -> PathBuf {
        self.target_repo_root.clone()
    }

    pub fn run_root(&self) -> PathBuf {
        self.resolve_target_path(&self.loop_cfg.run_root)
    }

    pub fn telemetry_history_path(&self) -> PathBuf {
        self.resolve_target_path(&self.telemetry.history_path)
    }

    pub fn resolve_target_path(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            normalize_path(path)
        } else {
            normalize_path(&self.target_repo_root.join(path))
        }
    }

    pub fn resolve_control_path(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            normalize_path(path)
        } else {
            normalize_path(&self.control_root.join(path))
        }
    }
}

pub fn resolve_config_path(
    cwd: &Path,
    explicit_config: Option<&Path>,
    target_name: Option<&str>,
) -> Result<PathBuf> {
    if let Some(path) = explicit_config {
        return Ok(resolve_invocation_path(cwd, path));
    }

    if let Some(target) = target_name {
        let path = cwd.join("targets").join(format!("{target}.toml"));
        if !path.exists() {
            bail!("target_config_not_found");
        }
        return Ok(path);
    }

    Ok(cwd.join("nightloop.toml"))
}

pub fn resolve_control_root(cwd: &Path, explicit_config: Option<&Path>) -> PathBuf {
    match explicit_config {
        Some(path) => {
            let anchor = resolve_invocation_path(cwd, path);
            normalize_path(anchor.parent().unwrap_or(cwd))
        }
        None => normalize_path(cwd),
    }
}

pub fn render_named_target_config(
    template: &str,
    repo_owner: &str,
    repo_name: &str,
    workdir: &Path,
    base_branch: &str,
) -> String {
    template
        .replacen(
            r#"owner = "your-org""#,
            &format!(r#"owner = "{}""#, escape_toml_string(repo_owner)),
            1,
        )
        .replacen(
            r#"repo = "your-repo""#,
            &format!(r#"repo = "{}""#, escape_toml_string(repo_name)),
            1,
        )
        .replacen(
            r#"base_branch = "main""#,
            &format!(r#"base_branch = "{}""#, escape_toml_string(base_branch)),
            1,
        )
        .replacen(
            r#"working_directory = ".""#,
            &format!(
                r#"working_directory = "{}""#,
                escape_toml_string(&workdir.display().to_string())
            ),
            1,
        )
}

fn default_copilot_reviewer() -> String {
    "github-copilot[bot]".to_string()
}

fn default_run_root() -> PathBuf {
    PathBuf::from(".nightloop/runs")
}

fn normalize_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn resolve_invocation_path(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

fn escape_toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use std::{env, fs};

    use super::{normalize_path, render_named_target_config, resolve_config_path, Config};

    #[test]
    fn resolves_control_target_run_and_telemetry_paths() {
        let root = env::temp_dir().join(format!("nightloop-config-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let control = root.join("control");
        let target = control.join("repos/project");
        fs::create_dir_all(&target).unwrap();
        let config_path = control.join("nightloop.toml");
        fs::write(
            &config_path,
            r#"[github]
owner = "o"
repo = "r"
base_branch = "main"

[agent]
command = "echo agent"
plan_command = "echo planner"
working_directory = "repos/project"
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
run_root = ".runs"

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
history_path = ".nightloop/history.jsonl"
min_samples_for_local = 1
local_weight = 0.65
template_weight = 0.35
"#,
        )
        .unwrap();

        let config = Config::load(&config_path).unwrap();
        assert_eq!(config.control_root(), normalize_path(&control));
        assert_eq!(config.target_repo_root(), normalize_path(&target));
        assert_eq!(config.run_root(), config.target_repo_root().join(".runs"));
        assert_eq!(
            config.telemetry_history_path(),
            config.target_repo_root().join(".nightloop/history.jsonl")
        );
        assert_eq!(
            config.resolve_control_path(std::path::Path::new("prompts/estimate_issue.md")),
            config.control_root().join("prompts/estimate_issue.md")
        );
        assert_eq!(
            config.resolve_target_path(std::path::Path::new("README.md")),
            config.target_repo_root().join("README.md")
        );
    }

    #[test]
    fn resolve_config_path_prefers_explicit_then_target_then_default() {
        let root = env::temp_dir().join(format!("nightloop-config-resolve-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("targets")).unwrap();
        fs::write(root.join("nightloop.toml"), "").unwrap();
        fs::write(root.join("targets/canaria.toml"), "").unwrap();
        fs::write(root.join("custom.toml"), "").unwrap();

        assert_eq!(
            resolve_config_path(
                &root,
                Some(std::path::Path::new("custom.toml")),
                Some("canaria")
            )
            .unwrap(),
            root.join("custom.toml")
        );
        assert_eq!(
            resolve_config_path(&root, None, Some("canaria")).unwrap(),
            root.join("targets/canaria.toml")
        );
        assert_eq!(
            resolve_config_path(&root, None, None).unwrap(),
            root.join("nightloop.toml")
        );
        assert_eq!(
            resolve_config_path(&root, None, Some("missing"))
                .unwrap_err()
                .to_string(),
            "target_config_not_found"
        );
    }

    #[test]
    fn render_named_target_config_reuses_template_shape() {
        let rendered = render_named_target_config(
            include_str!("../nightloop.example.toml"),
            "UTAGEDA",
            "canaria",
            std::path::Path::new("/tmp/canaria"),
            "develop",
        );
        assert!(rendered.contains(r#"owner = "UTAGEDA""#));
        assert!(rendered.contains(r#"repo = "canaria""#));
        assert!(rendered.contains(r#"base_branch = "develop""#));
        assert!(rendered.contains(r#"working_directory = "/tmp/canaria""#));
        assert!(rendered.contains(r#"request_copilot_review = false"#));
    }
}
