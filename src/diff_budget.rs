use anyhow::{anyhow, bail, Result};

use crate::{config::Config, models::ChildIssue};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffStat {
    pub changed_lines: u32,
    pub files_touched: u32,
}

pub fn parse_numstat(output: &str) -> Result<DiffStat> {
    let mut changed_lines = 0u32;
    let mut files_touched = 0u32;

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut parts = trimmed.split('\t');
        let added = parts
            .next()
            .ok_or_else(|| anyhow!("invalid numstat line: {trimmed}"))?;
        let removed = parts
            .next()
            .ok_or_else(|| anyhow!("invalid numstat line: {trimmed}"))?;
        let _path = parts
            .next()
            .ok_or_else(|| anyhow!("invalid numstat line: {trimmed}"))?;

        if added != "-" {
            changed_lines += added.parse::<u32>()?;
        }
        if removed != "-" {
            changed_lines += removed.parse::<u32>()?;
        }
        files_touched += 1;
    }

    Ok(DiffStat {
        changed_lines,
        files_touched,
    })
}

pub fn enforce_diff_budget(config: &Config, child: &ChildIssue, stat: DiffStat) -> Result<()> {
    if stat.changed_lines > config.diff.max_lines {
        bail!("diff_exceeds_global_max");
    }
    if stat.changed_lines > child.target_size.max_lines() {
        bail!("diff_exceeds_target_size");
    }

    if stat.changed_lines < config.diff.min_lines
        || stat.changed_lines < child.target_size.min_lines()
    {
        if config.diff.allow_doc_only_below_min && child.allows_small_diff_exception() {
            return Ok(());
        }
        bail!("diff_below_minimum");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{
        config::Config,
        models::{ChildIssue, Confidence, DocsImpact, EstimationBasis, IssueState, SizeBand},
    };

    use super::{enforce_diff_budget, parse_numstat, DiffStat};

    fn child(scope: &str) -> ChildIssue {
        ChildIssue {
            number: 1,
            title: "title".to_string(),
            body: String::new(),
            state: IssueState::Open,
            labels: Vec::new(),
            url: None,
            sections: Default::default(),
            background: String::new(),
            goal: String::new(),
            scope: scope.to_string(),
            out_of_scope: String::new(),
            source_of_truth_raw: String::new(),
            source_of_truth: Vec::new(),
            implementation_constraints: None,
            acceptance_criteria: String::new(),
            verification_raw: String::new(),
            verification: Vec::new(),
            dependencies_raw: String::new(),
            dependencies: Vec::new(),
            target_size: SizeBand::Xs,
            docs_impact: DocsImpact::None,
            suggested_model_profile: "balanced".to_string(),
            suggested_model_override: None,
            estimated_minutes: 30,
            estimation_basis: EstimationBasis::Template,
            estimation_confidence: Confidence::Medium,
        }
    }

    fn config() -> Config {
        toml::from_str(
            r#"[github]
owner = "o"
repo = "r"
base_branch = "main"

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
required_paths = []

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
        )
        .unwrap()
    }

    #[test]
    fn numstat_parser_sums_changed_lines() {
        let stat = parse_numstat("10\t5\tsrc/main.rs\n3\t2\tREADME.md\n").unwrap();
        assert_eq!(
            stat,
            DiffStat {
                changed_lines: 20,
                files_touched: 2
            }
        );
    }

    #[test]
    fn below_minimum_requires_explicit_scope_marker() {
        assert!(enforce_diff_budget(
            &config(),
            &child("normal"),
            DiffStat {
                changed_lines: 20,
                files_touched: 1
            }
        )
        .is_err());
        assert!(enforce_diff_budget(
            &config(),
            &child("docs-only"),
            DiffStat {
                changed_lines: 20,
                files_touched: 1
            }
        )
        .is_ok());
    }
}
