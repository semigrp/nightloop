use std::path::PathBuf;

use anyhow::Result;

use crate::config::Config;

#[derive(Debug)]
pub struct DocsReport {
    pub ok: bool,
    pub missing_paths: Vec<PathBuf>,
}

pub fn check_docs(config: &Config) -> Result<DocsReport> {
    let missing_paths = config
        .docs
        .required_paths
        .iter()
        .filter(|path| !path.exists())
        .cloned()
        .collect::<Vec<_>>();

    Ok(DocsReport {
        ok: missing_paths.is_empty(),
        missing_paths,
    })
}
