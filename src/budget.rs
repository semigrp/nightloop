use anyhow::{bail, Result};

#[derive(Debug, Clone, Copy)]
pub struct BudgetReport {
    pub hours: u32,
    pub available_minutes: u32,
    pub fixed_overhead_minutes: u32,
    pub fallback_cycle_minutes: u32,
    pub fallback_slots: u32,
}

pub fn slots_for_hours(
    hours: u32,
    cycle_minutes: u32,
    fixed_overhead_minutes: u32,
    min_hours: u32,
    max_hours: u32,
) -> Result<u32> {
    let available_minutes = available_minutes(hours, fixed_overhead_minutes, min_hours, max_hours)?;
    if cycle_minutes == 0 {
        bail!("cycle_minutes must be > 0");
    }
    Ok(available_minutes / cycle_minutes)
}

pub fn available_minutes(
    hours: u32,
    fixed_overhead_minutes: u32,
    min_hours: u32,
    max_hours: u32,
) -> Result<u32> {
    if !(min_hours..=max_hours).contains(&hours) {
        bail!("hours must be between {min_hours} and {max_hours}");
    }
    let total = hours * 60;
    if total <= fixed_overhead_minutes {
        return Ok(0);
    }
    Ok(total - fixed_overhead_minutes)
}

pub fn budget_report(
    hours: u32,
    cycle_minutes: u32,
    fixed_overhead_minutes: u32,
    min_hours: u32,
    max_hours: u32,
) -> Result<BudgetReport> {
    let available_minutes = available_minutes(hours, fixed_overhead_minutes, min_hours, max_hours)?;
    let fallback_slots = slots_for_hours(
        hours,
        cycle_minutes,
        fixed_overhead_minutes,
        min_hours,
        max_hours,
    )?;
    Ok(BudgetReport {
        hours,
        available_minutes,
        fixed_overhead_minutes,
        fallback_cycle_minutes: cycle_minutes,
        fallback_slots,
    })
}

#[cfg(test)]
mod tests {
    use super::{available_minutes, budget_report, slots_for_hours};

    #[test]
    fn fallback_capacity_matches_default_table() {
        assert_eq!(slots_for_hours(2, 40, 20, 2, 6).unwrap(), 2);
        assert_eq!(slots_for_hours(3, 40, 20, 2, 6).unwrap(), 4);
        assert_eq!(slots_for_hours(4, 40, 20, 2, 6).unwrap(), 5);
        assert_eq!(slots_for_hours(5, 40, 20, 2, 6).unwrap(), 7);
        assert_eq!(slots_for_hours(6, 40, 20, 2, 6).unwrap(), 8);
    }

    #[test]
    fn available_minutes_respects_fixed_overhead() {
        assert_eq!(available_minutes(4, 20, 2, 6).unwrap(), 220);
        let report = budget_report(4, 40, 20, 2, 6).unwrap();
        assert_eq!(report.available_minutes, 220);
    }
}
