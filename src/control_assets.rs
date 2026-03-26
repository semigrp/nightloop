use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathSpec {
    pub kind: &'static str,
    pub path: PathBuf,
}

impl PathSpec {
    pub fn new(kind: &'static str, path: &str) -> Self {
        Self {
            kind,
            path: PathBuf::from(path),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ControlAssetManifest {
    pub runtime_required_paths: Vec<PathSpec>,
    pub authoring_required_paths: Vec<PathSpec>,
    pub plan_prompt: PathSpec,
    pub implement_prompt: Option<PathSpec>,
    pub plan_template: Option<PathSpec>,
}

pub fn manifest() -> ControlAssetManifest {
    let plan_prompt = PathSpec::new("prompt", "prompts/plan_child_issue.md");
    let plan_template = PathSpec::new("template", "docs/templates/plan.md");

    ControlAssetManifest {
        runtime_required_paths: vec![plan_prompt.clone()],
        authoring_required_paths: vec![plan_template.clone()],
        plan_prompt,
        implement_prompt: None,
        plan_template: Some(plan_template),
    }
}
