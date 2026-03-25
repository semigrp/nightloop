use std::{
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::Path,
};

use anyhow::{Context, Result};

use crate::models::{DocsImpact, RunRecord, SizeBand};

#[derive(Debug, Default, Clone, Copy)]
pub struct LocalStats {
    pub samples: usize,
    pub average_minutes: f32,
}

pub fn read_run_record(path: &Path) -> Result<RunRecord> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read run record from {}", path.display()))?;
    let record = serde_json::from_str::<RunRecord>(&raw)
        .with_context(|| format!("failed to parse JSON run record from {}", path.display()))?;
    Ok(record)
}

pub fn append_run_record(history_path: &Path, record: &RunRecord) -> Result<()> {
    if let Some(parent) = history_path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!("failed to create telemetry directory {}", parent.display())
        })?;
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(history_path)
        .with_context(|| {
            format!(
                "failed to open telemetry history {}",
                history_path.display()
            )
        })?;

    let line = serde_json::to_string(record)?;
    writeln!(file, "{line}")?;
    Ok(())
}

pub fn load_stats(
    history_path: &Path,
    model_profile: &str,
    size_band: &SizeBand,
    docs_impact: &DocsImpact,
) -> Result<LocalStats> {
    if !history_path.exists() {
        return Ok(LocalStats::default());
    }

    let file = fs::File::open(history_path).with_context(|| {
        format!(
            "failed to open telemetry history {}",
            history_path.display()
        )
    })?;
    let reader = BufReader::new(file);

    let mut total = 0u64;
    let mut samples = 0usize;

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<RunRecord>(&line) else {
            continue;
        };
        if !record.success {
            continue;
        }
        if record.model_profile.eq_ignore_ascii_case(model_profile)
            && &record.target_size == size_band
            && &record.docs_impact == docs_impact
        {
            total += record.actual_minutes as u64;
            samples += 1;
        }
    }

    if samples == 0 {
        return Ok(LocalStats::default());
    }

    Ok(LocalStats {
        samples,
        average_minutes: total as f32 / samples as f32,
    })
}

#[cfg(test)]
mod tests {
    use std::{env, fs};

    use chrono::Utc;

    use crate::models::{DocsImpact, RunRecord, SizeBand};

    use super::{append_run_record, load_stats, read_run_record};

    fn record(actual_minutes: u32) -> RunRecord {
        RunRecord {
            run_id: "run-1".to_string(),
            parent_issue: 10,
            issue_number: 11,
            issue_title: "title".to_string(),
            model_profile: "balanced".to_string(),
            model: "gpt-5.4".to_string(),
            reasoning_effort: "medium".to_string(),
            target_size: SizeBand::M,
            docs_impact: DocsImpact::None,
            estimated_minutes: 80,
            actual_minutes,
            changed_lines: 220,
            files_touched: 3,
            success: true,
            status: "success".to_string(),
            branch: "nightloop/10-11".to_string(),
            pr_base: "main".to_string(),
            pr_url: None,
            recorded_at: Utc::now(),
        }
    }

    #[test]
    fn append_and_read_run_record_round_trip() {
        let root = env::temp_dir().join(format!("nightloop-telemetry-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let record_path = root.join("record.json");
        fs::write(&record_path, serde_json::to_string(&record(90)).unwrap()).unwrap();
        let parsed = read_run_record(&record_path).unwrap();
        assert_eq!(parsed.issue_number, 11);
        let history_path = root.join("history.jsonl");
        append_run_record(&history_path, &parsed).unwrap();
        let stats = load_stats(&history_path, "balanced", &SizeBand::M, &DocsImpact::None).unwrap();
        assert_eq!(stats.samples, 1);
        assert_eq!(stats.average_minutes, 90.0);
    }
}
