use std::collections::HashSet;

use crate::{config::Config, models::ChildIssue};

pub fn static_eligibility_reasons(config: &Config, child: &ChildIssue) -> Vec<String> {
    let mut reasons = Vec::new();
    if child.state.as_str() != "open" {
        reasons.push("issue_closed".to_string());
    }
    if !child.has_label(&config.labels.night_run) {
        reasons.push("label_missing_night_run".to_string());
    }
    if !child.has_label(&config.labels.ready) {
        reasons.push("label_missing_ready".to_string());
    }
    if child.has_label(&config.labels.running) {
        reasons.push("label_present_running".to_string());
    }
    if child.has_label(&config.labels.blocked) {
        reasons.push("label_present_blocked".to_string());
    }
    if child.has_label(&config.labels.done) {
        reasons.push("label_present_done".to_string());
    }
    if child.has_label(&config.labels.review) {
        reasons.push("label_present_review".to_string());
    }
    if child.target_size.max_lines() > config.diff.max_lines {
        reasons.push("size_band_exceeds_diff_max".to_string());
    }
    if child.target_size.min_lines() < config.diff.min_lines {
        reasons.push("size_band_below_diff_min".to_string());
    }
    reasons
}

pub fn dependencies_satisfied(
    child: &ChildIssue,
    done_on_github: &HashSet<u64>,
    planned_or_completed: &HashSet<u64>,
) -> bool {
    child.dependencies.iter().all(|dependency| {
        done_on_github.contains(dependency) || planned_or_completed.contains(dependency)
    })
}

pub fn pack_issue_if_fit(
    estimated_minutes: u32,
    used_minutes: u32,
    available_minutes: u32,
) -> bool {
    used_minutes + estimated_minutes <= available_minutes
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crate::{
        config::Config,
        models::{ChildIssue, Confidence, DocsImpact, EstimationBasis, IssueState, SizeBand},
    };

    use super::{dependencies_satisfied, pack_issue_if_fit, static_eligibility_reasons};

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
        .unwrap()
    }

    fn issue(labels: &[&str], dependencies: Vec<u64>) -> ChildIssue {
        ChildIssue {
            number: 1,
            title: "title".to_string(),
            body: String::new(),
            state: IssueState::Open,
            labels: labels.iter().map(|value| value.to_string()).collect(),
            url: None,
            sections: Default::default(),
            background: String::new(),
            goal: String::new(),
            scope: String::new(),
            out_of_scope: String::new(),
            source_of_truth_raw: String::new(),
            source_of_truth: Vec::new(),
            implementation_constraints: None,
            acceptance_criteria: String::new(),
            verification_raw: String::new(),
            verification: Vec::new(),
            dependencies_raw: String::new(),
            dependencies,
            target_size: SizeBand::M,
            docs_impact: DocsImpact::None,
            suggested_model_profile: "balanced".to_string(),
            suggested_model_override: None,
            estimated_minutes: 80,
            estimation_basis: EstimationBasis::Template,
            estimation_confidence: Confidence::Medium,
        }
    }

    #[test]
    fn static_reasons_capture_label_gaps() {
        let reasons = static_eligibility_reasons(&config(), &issue(&["night-run"], vec![]));
        assert!(reasons.iter().any(|reason| reason == "label_missing_ready"));
    }

    #[test]
    fn dependency_satisfaction_respects_done_and_planned_sets() {
        let child = issue(&["night-run", "agent:ready"], vec![2, 3]);
        let done = HashSet::from([2]);
        let planned = HashSet::from([3]);
        assert!(dependencies_satisfied(&child, &done, &planned));
    }

    #[test]
    fn packing_respects_available_budget() {
        assert!(pack_issue_if_fit(60, 100, 200));
        assert!(!pack_issue_if_fit(120, 100, 200));
    }
}
