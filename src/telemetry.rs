use std::{
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::Path,
};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::models::{DocsImpact, SizeBand};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub issue_number: u64,
    pub model_profile: String,
    pub model: String,
    pub reasoning_effort: String,
    pub target_size: SizeBand,
    pub docs_impact: DocsImpact,
    pub estimated_minutes: Option<u32>,
    pub actual_minutes: u32,
    pub changed_lines: u32,
    pub files_touched: u32,
    pub success: bool,
    pub recorded_at: DateTime<Utc>,
}

#[derive(Debug, Default)]
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
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create telemetry directory {}", parent.display()))?;
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(history_path)
        .with_context(|| format!("failed to open telemetry history {}", history_path.display()))?;

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

    let file = fs::File::open(history_path)
        .with_context(|| format!("failed to open telemetry history {}", history_path.display()))?;
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
