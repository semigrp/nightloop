use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcomeKind {
    Success,
    PartialSuccess,
    Blocked,
    SplitRequired,
    Aborted,
    RetryableFailure,
}

impl RunOutcomeKind {
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunOutcome {
    pub kind: RunOutcomeKind,
    pub status: String,
    pub reason: Option<String>,
    pub actual_minutes: Option<u32>,
    pub changed_lines: Option<u32>,
    pub files_touched: Option<u32>,
    pub pr_url: Option<String>,
}

impl RunOutcome {
    pub fn success(
        actual_minutes: u32,
        changed_lines: u32,
        files_touched: u32,
        pr_url: String,
    ) -> Self {
        Self {
            kind: RunOutcomeKind::Success,
            status: "success".to_string(),
            reason: None,
            actual_minutes: Some(actual_minutes),
            changed_lines: Some(changed_lines),
            files_touched: Some(files_touched),
            pr_url: Some(pr_url),
        }
    }

    pub fn terminal(
        kind: RunOutcomeKind,
        status: &str,
        reason: String,
        actual_minutes: u32,
        changed_lines: Option<u32>,
        files_touched: Option<u32>,
    ) -> Self {
        Self {
            kind,
            status: status.to_string(),
            reason: Some(reason),
            actual_minutes: Some(actual_minutes),
            changed_lines,
            files_touched,
            pr_url: None,
        }
    }
}
