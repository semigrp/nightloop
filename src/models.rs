use std::{collections::BTreeMap, path::PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum IssueState {
    Open,
    Closed,
}

impl IssueState {
    pub fn from_github_state(value: &str) -> Self {
        if value.eq_ignore_ascii_case("closed") {
            Self::Closed
        } else {
            Self::Open
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SizeBand {
    Xs,
    S,
    M,
    L,
}

impl SizeBand {
    pub fn from_text(value: &str) -> Option<Self> {
        match value.trim().to_ascii_uppercase().as_str() {
            "XS" | "XS (50-120)" => Some(Self::Xs),
            "S" | "S (120-250)" => Some(Self::S),
            "M" | "M (250-500)" => Some(Self::M),
            "L" | "L (500-1000)" => Some(Self::L),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Xs => "XS",
            Self::S => "S",
            Self::M => "M",
            Self::L => "L",
        }
    }

    pub fn min_lines(&self) -> u32 {
        match self {
            Self::Xs => 50,
            Self::S => 120,
            Self::M => 250,
            Self::L => 500,
        }
    }

    pub fn max_lines(&self) -> u32 {
        match self {
            Self::Xs => 120,
            Self::S => 250,
            Self::M => 500,
            Self::L => 1000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DocsImpact {
    None,
    Readme,
    UserFacingDocs,
    ArchitectureDocs,
}

impl DocsImpact {
    pub fn from_text(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "none" => Some(Self::None),
            "readme" => Some(Self::Readme),
            "user-facing-docs" => Some(Self::UserFacingDocs),
            "architecture-docs" => Some(Self::ArchitectureDocs),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Readme => "readme",
            Self::UserFacingDocs => "user-facing-docs",
            Self::ArchitectureDocs => "architecture-docs",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EstimationBasis {
    Template,
    Local,
    Hybrid,
    Ai,
    Manual,
}

impl EstimationBasis {
    pub fn from_text(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "template" => Some(Self::Template),
            "local" => Some(Self::Local),
            "hybrid" => Some(Self::Hybrid),
            "ai" => Some(Self::Ai),
            "manual" => Some(Self::Manual),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Template => "template",
            Self::Local => "local",
            Self::Hybrid => "hybrid",
            Self::Ai => "ai",
            Self::Manual => "manual",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    Low,
    Medium,
    High,
}

impl Confidence {
    pub fn from_text(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueSnapshot {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub state: IssueState,
    pub labels: Vec<String>,
    pub url: Option<String>,
}

impl IssueSnapshot {
    pub fn has_label(&self, value: &str) -> bool {
        self.labels
            .iter()
            .any(|label| label.eq_ignore_ascii_case(value))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParentChildRef {
    pub number: u64,
    pub checked: bool,
    pub raw_line: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParentIssue {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub state: IssueState,
    pub labels: Vec<String>,
    pub url: Option<String>,
    pub sections: BTreeMap<String, String>,
    pub children: Vec<ParentChildRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceRefKind {
    RepoRelative { path: PathBuf },
    Absolute { path: PathBuf },
    Url { url: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceRef {
    pub raw: String,
    pub kind: SourceRefKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerificationSource {
    FencedShell,
    CmdLine,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationCommand {
    pub command: String,
    pub source: VerificationSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChildIssue {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub state: IssueState,
    pub labels: Vec<String>,
    pub url: Option<String>,
    pub sections: BTreeMap<String, String>,
    pub background: String,
    pub goal: String,
    pub scope: String,
    pub out_of_scope: String,
    pub source_of_truth_raw: String,
    pub source_of_truth: Vec<SourceRef>,
    pub implementation_constraints: Option<String>,
    pub acceptance_criteria: String,
    pub verification_raw: String,
    pub verification: Vec<VerificationCommand>,
    pub dependencies_raw: String,
    pub dependencies: Vec<u64>,
    pub target_size: SizeBand,
    pub docs_impact: DocsImpact,
    pub suggested_model_profile: String,
    pub suggested_model_override: Option<String>,
    pub estimated_minutes: u32,
    pub estimation_basis: EstimationBasis,
    pub estimation_confidence: Confidence,
}

impl ChildIssue {
    pub fn has_label(&self, value: &str) -> bool {
        self.labels
            .iter()
            .any(|label| label.eq_ignore_ascii_case(value))
    }

    pub fn allows_small_diff_exception(&self) -> bool {
        self.scope.lines().any(|line| {
            let trimmed = line.trim();
            trimmed == "docs-only" || trimmed == "config-only"
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiEstimate {
    pub model_profile: String,
    pub estimated_minutes: u32,
    pub confidence: Confidence,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueEstimate {
    pub model_profile: String,
    pub model: String,
    pub reasoning_effort: String,
    pub estimated_minutes: u32,
    pub recommended_hours: u32,
    pub basis_requested: String,
    pub basis_used: String,
    pub local_samples: usize,
    pub notes: Vec<String>,
    pub ai_estimate: Option<AiEstimate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub run_id: String,
    pub parent_issue: u64,
    pub issue_number: u64,
    pub issue_title: String,
    pub model_profile: String,
    pub model: String,
    pub reasoning_effort: String,
    pub target_size: SizeBand,
    pub docs_impact: DocsImpact,
    pub estimated_minutes: u32,
    pub actual_minutes: u32,
    pub changed_lines: u32,
    pub files_touched: u32,
    pub success: bool,
    pub status: String,
    pub copilot_review: Option<String>,
    pub branch: String,
    pub pr_base: String,
    pub pr_url: Option<String>,
    pub recorded_at: DateTime<Utc>,
}
