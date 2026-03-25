use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChildIssue {
    pub number: u64,
    pub title: String,
    pub labels: Vec<String>,
    pub dependencies: Vec<u64>,
    pub target_size: SizeBand,
    pub docs_impact: DocsImpact,
    pub suggested_model_profile: Option<String>,
    pub suggested_model_override: Option<String>,
    pub estimated_minutes: Option<u32>,
    pub estimation_basis: Option<EstimationBasis>,
    pub estimation_confidence: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SizeBand {
    Xs,
    S,
    M,
    L,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DocsImpact {
    None,
    Readme,
    UserFacing,
    Architecture,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum EstimationBasis {
    Template,
    Local,
    Hybrid,
    Ai,
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CampaignRun {
    pub parent_issue: u64,
    pub started_at: DateTime<Utc>,
    pub selected_children: Vec<u64>,
}
