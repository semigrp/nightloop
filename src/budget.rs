use anyhow::{bail, Result};

pub fn slots_for_hours(
    hours: u32,
    cycle_minutes: u32,
    fixed_overhead_minutes: u32,
    min_hours: u32,
    max_hours: u32,
) -> Result<u32> {
    if !(min_hours..=max_hours).contains(&hours) {
        bail!("hours must be between {min_hours} and {max_hours}");
    }
    if cycle_minutes == 0 {
        bail!("cycle_minutes must be > 0");
    }
    let total = hours * 60;
    if total <= fixed_overhead_minutes {
        return Ok(0);
    }
    Ok((total - fixed_overhead_minutes) / cycle_minutes)
}

pub fn estimated_issues_that_fit(
    hours: u32,
    fixed_overhead_minutes: u32,
    estimated_minutes: &[u32],
    min_hours: u32,
    max_hours: u32,
) -> Result<usize> {
    if !(min_hours..=max_hours).contains(&hours) {
        bail!("hours must be between {min_hours} and {max_hours}");
    }
    let mut used = fixed_overhead_minutes;
    let budget = hours * 60;
    let mut count = 0usize;

    for minutes in estimated_minutes {
        if used + minutes > budget {
            break;
        }
        used += minutes;
        count += 1;
    }

    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::{estimated_issues_that_fit, slots_for_hours};

    #[test]
    fn fallback_capacity_table_matches_previous_default() {
        assert_eq!(slots_for_hours(2, 40, 20, 2, 6).unwrap(), 2);
        assert_eq!(slots_for_hours(3, 40, 20, 2, 6).unwrap(), 4);
        assert_eq!(slots_for_hours(4, 40, 20, 2, 6).unwrap(), 5);
        assert_eq!(slots_for_hours(5, 40, 20, 2, 6).unwrap(), 7);
        assert_eq!(slots_for_hours(6, 40, 20, 2, 6).unwrap(), 8);
    }

    #[test]
    fn estimated_packing_uses_issue_specific_minutes() {
        let estimated = vec![35, 50, 80, 120];
        assert_eq!(estimated_issues_that_fit(2, 20, &estimated, 2, 6).unwrap(), 2);
        assert_eq!(estimated_issues_that_fit(4, 20, &estimated, 2, 6).unwrap(), 3);
    }
}
