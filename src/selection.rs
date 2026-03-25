use crate::models::ChildIssue;

/// Select runnable child Issues in dependency order.
///
/// v0 policy:
/// - only Issues already tagged with night-run and agent:ready are eligible
/// - skip any Issue whose dependencies are not in `completed`
/// - preserve the incoming order from the parent issue checklist or explicit priority
pub fn select_runnable<'a>(issues: &'a [ChildIssue], completed: &[u64]) -> Vec<&'a ChildIssue> {
    issues
        .iter()
        .filter(|issue| issue.dependencies.iter().all(|d| completed.contains(d)))
        .collect()
}

/// Pack already-estimated Issues into a single nightly window.
///
/// This intentionally preserves order. v0 should prefer predictability over optimal packing.
pub fn pack_by_estimate<'a>(issues: &'a [&'a ChildIssue], available_minutes: u32) -> Vec<&'a ChildIssue> {
    let mut used = 0u32;
    let mut packed = Vec::new();

    for issue in issues {
        let Some(estimated) = issue.estimated_minutes else {
            break;
        };
        if used + estimated > available_minutes {
            break;
        }
        used += estimated;
        packed.push(*issue);
    }

    packed
}
