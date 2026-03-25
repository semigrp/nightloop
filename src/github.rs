use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;

use crate::{
    agent_exec,
    config::Config,
    models::{IssueSnapshot, IssueState},
};

pub struct GitHubClient<'a> {
    config: &'a Config,
}

impl<'a> GitHubClient<'a> {
    pub fn new(config: &'a Config) -> Self {
        Self { config }
    }

    pub fn check_auth(&self) -> Result<()> {
        let result = agent_exec::run_shell_command(
            "gh auth status",
            &self.config.working_directory(),
            &[],
            None,
        )?;
        if result.success() {
            return Ok(());
        }
        bail!("gh_auth_invalid");
    }

    pub fn view_issue(&self, issue_number: u64) -> Result<IssueSnapshot> {
        let command = format!(
            "gh issue view {} --repo {} --json number,title,body,state,labels,url",
            issue_number,
            self.config.repo_slug()
        );
        let result =
            agent_exec::run_shell_command(&command, &self.config.working_directory(), &[], None)?;
        if !result.success() {
            bail!("gh_issue_view_failed");
        }
        parse_issue_json(&result.stdout)
    }

    pub fn add_labels(&self, issue_number: u64, labels: &[&str]) -> Result<()> {
        if labels.is_empty() {
            return Ok(());
        }
        let label_args = labels
            .iter()
            .map(|label| format!("--add-label {}", shell_quote(label)))
            .collect::<Vec<_>>()
            .join(" ");
        let command = format!(
            "gh issue edit {} --repo {} {}",
            issue_number,
            self.config.repo_slug(),
            label_args
        );
        let result =
            agent_exec::run_shell_command(&command, &self.config.working_directory(), &[], None)?;
        if !result.success() {
            bail!("gh_label_update_failed");
        }
        Ok(())
    }

    pub fn remove_labels(&self, issue_number: u64, labels: &[&str]) -> Result<()> {
        if labels.is_empty() {
            return Ok(());
        }
        let label_args = labels
            .iter()
            .map(|label| format!("--remove-label {}", shell_quote(label)))
            .collect::<Vec<_>>()
            .join(" ");
        let command = format!(
            "gh issue edit {} --repo {} {}",
            issue_number,
            self.config.repo_slug(),
            label_args
        );
        let result =
            agent_exec::run_shell_command(&command, &self.config.working_directory(), &[], None)?;
        if !result.success() {
            bail!("gh_label_update_failed");
        }
        Ok(())
    }

    pub fn comment_issue(&self, issue_number: u64, body: &str) -> Result<()> {
        let command = format!(
            "gh issue comment {} --repo {} --body {}",
            issue_number,
            self.config.repo_slug(),
            shell_quote(body)
        );
        let result =
            agent_exec::run_shell_command(&command, &self.config.working_directory(), &[], None)?;
        if !result.success() {
            bail!("gh_issue_comment_failed");
        }
        Ok(())
    }

    pub fn create_draft_pr(
        &self,
        base: &str,
        head: &str,
        title: &str,
        body: &str,
    ) -> Result<String> {
        let command = format!(
            "gh pr create --repo {} --draft --base {} --head {} --title {} --body {}",
            self.config.repo_slug(),
            shell_quote(base),
            shell_quote(head),
            shell_quote(title),
            shell_quote(body)
        );
        let result =
            agent_exec::run_shell_command(&command, &self.config.working_directory(), &[], None)?;
        if !result.success() {
            bail!("gh_pr_create_failed");
        }
        let url = result
            .stdout
            .lines()
            .find(|line| line.starts_with("http://") || line.starts_with("https://"))
            .ok_or_else(|| anyhow!("gh_pr_create_missing_url"))?;
        Ok(url.to_string())
    }
}

#[derive(Debug, Deserialize)]
struct IssueJson {
    number: u64,
    title: String,
    body: String,
    state: String,
    labels: Vec<LabelJson>,
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LabelJson {
    name: String,
}

pub fn parse_issue_json(raw: &str) -> Result<IssueSnapshot> {
    let parsed = serde_json::from_str::<IssueJson>(raw).context("failed to parse gh issue json")?;
    Ok(IssueSnapshot {
        number: parsed.number,
        title: parsed.title,
        body: parsed.body,
        state: IssueState::from_github_state(&parsed.state),
        labels: parsed.labels.into_iter().map(|label| label.name).collect(),
        url: parsed.url,
    })
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::parse_issue_json;

    #[test]
    fn parses_issue_fixture_json() {
        let issue = parse_issue_json(include_str!("../tests/fixtures/github_issue.json")).unwrap();
        assert_eq!(issue.number, 221);
        assert!(issue.has_label("campaign"));
    }
}
