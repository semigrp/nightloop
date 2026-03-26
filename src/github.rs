use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use std::collections::HashSet;

use crate::{
    agent_exec::{self, CommandRunOptions},
    config::{Config, Labels},
    models::{IssueSnapshot, IssueState},
};

pub struct GitHubClient<'a> {
    config: &'a Config,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedLabelStatus {
    pub name: String,
    pub status: String,
    pub color: Option<String>,
    pub description: String,
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
            CommandRunOptions::streaming("gh"),
        )?;
        if result.success() {
            Ok(())
        } else {
            bail!("gh_auth_invalid")
        }
    }

    pub fn view_issue(&self, issue_number: u64) -> Result<IssueSnapshot> {
        let command = format!(
            "gh issue view {} --repo {} --json number,title,body,state,labels,url",
            issue_number,
            self.config.repo_slug()
        );
        let result = agent_exec::run_shell_command(
            &command,
            &self.config.working_directory(),
            &[],
            CommandRunOptions::streaming("gh"),
        )?;
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
        let result = agent_exec::run_shell_command(
            &command,
            &self.config.working_directory(),
            &[],
            CommandRunOptions::streaming("gh"),
        )?;
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
        let result = agent_exec::run_shell_command(
            &command,
            &self.config.working_directory(),
            &[],
            CommandRunOptions::streaming("gh"),
        )?;
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
        let result = agent_exec::run_shell_command(
            &command,
            &self.config.working_directory(),
            &[],
            CommandRunOptions::streaming("gh"),
        )?;
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
        let result = agent_exec::run_shell_command(
            &command,
            &self.config.working_directory(),
            &[],
            CommandRunOptions::streaming("gh"),
        )?;
        if !result.success() {
            bail!("gh_pr_create_failed");
        }
        result
            .stdout
            .lines()
            .find(|line| line.starts_with("http://") || line.starts_with("https://"))
            .map(ToString::to_string)
            .ok_or_else(|| anyhow!("gh_pr_create_missing_url"))
    }

    pub fn list_labels(&self) -> Result<Vec<String>> {
        let command = format!(
            "gh label list --repo {} --json name",
            self.config.repo_slug()
        );
        let result = agent_exec::run_shell_command(
            &command,
            &self.config.working_directory(),
            &[],
            CommandRunOptions::streaming("gh"),
        )?;
        if !result.success() {
            bail!("gh_label_list_failed");
        }
        parse_label_list_json(&result.stdout)
    }

    pub fn create_label(&self, name: &str, color: &str, description: &str) -> Result<()> {
        let command =
            build_create_label_command(&self.config.repo_slug(), name, color, description);
        let result = agent_exec::run_shell_command(
            &command,
            &self.config.working_directory(),
            &[],
            CommandRunOptions::streaming("gh"),
        )?;
        if !result.success() {
            bail!("gh_label_create_failed");
        }
        Ok(())
    }

    pub fn ensure_managed_labels(&self) -> Result<Vec<ManagedLabelStatus>> {
        let existing = self.list_labels()?.into_iter().collect::<HashSet<_>>();
        let plan = reconcile_managed_labels(&existing, &self.config.labels);
        for item in &plan {
            if item.status == "created" {
                self.create_label(
                    &item.name,
                    item.color.as_deref().unwrap_or(""),
                    &item.description,
                )?;
            }
        }
        Ok(plan)
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

#[derive(Debug, Deserialize)]
struct RepoLabelJson {
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

pub fn parse_label_list_json(raw: &str) -> Result<Vec<String>> {
    let parsed =
        serde_json::from_str::<Vec<RepoLabelJson>>(raw).context("failed to parse gh label json")?;
    Ok(parsed.into_iter().map(|label| label.name).collect())
}

pub fn build_create_label_command(
    repo_slug: &str,
    name: &str,
    color: &str,
    description: &str,
) -> String {
    format!(
        "gh label create {} --repo {} --color {} --description {}",
        shell_quote(name),
        repo_slug,
        shell_quote(color),
        shell_quote(description)
    )
}

pub fn reconcile_managed_labels(
    existing: &HashSet<String>,
    labels: &Labels,
) -> Vec<ManagedLabelStatus> {
    managed_labels(labels)
        .into_iter()
        .map(|(name, color, description)| ManagedLabelStatus {
            name: name.clone(),
            status: if existing.contains(&name) {
                "exists".to_string()
            } else {
                "created".to_string()
            },
            color: Some(color.to_string()),
            description: description.to_string(),
        })
        .collect()
}

fn managed_labels(labels: &Labels) -> Vec<(String, &'static str, &'static str)> {
    vec![
        (labels.campaign.clone(), "1d76db", "Parent campaign issue"),
        (
            labels.night_run.clone(),
            "0e8a16",
            "Eligible for runner selection",
        ),
        (labels.ready.clone(), "1d76db", "Ready for agent execution"),
        (labels.running.clone(), "fbca04", "Currently executing"),
        (labels.review.clone(), "5319e7", "Draft PR open for review"),
        (labels.blocked.clone(), "d93f0b", "Execution blocked"),
        (labels.done.clone(), "0e8a16", "Completed and merged"),
    ]
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crate::config::Labels;

    use super::{
        build_create_label_command, parse_issue_json, parse_label_list_json,
        reconcile_managed_labels,
    };

    #[test]
    fn parses_issue_snapshot() {
        let issue = parse_issue_json(include_str!("../tests/fixtures/github_issue.json")).unwrap();
        assert_eq!(issue.number, 221);
        assert!(issue.labels.iter().any(|label| label == "night-run"));
    }

    #[test]
    fn parses_repo_labels() {
        let labels =
            parse_label_list_json(include_str!("../tests/fixtures/github_labels.json")).unwrap();
        assert!(labels.iter().any(|label| label == "agent:ready"));
    }

    #[test]
    fn build_create_label_command_quotes_inputs() {
        let command = build_create_label_command("o/r", "agent:ready", "ffffff", "Ready");
        assert!(command.contains("gh label create 'agent:ready' --repo o/r"));
    }

    #[test]
    fn reconcile_managed_labels_marks_existing_and_missing() {
        let labels = Labels {
            campaign: "campaign".to_string(),
            night_run: "night-run".to_string(),
            ready: "agent:ready".to_string(),
            running: "agent:running".to_string(),
            review: "agent:review".to_string(),
            blocked: "agent:blocked".to_string(),
            done: "agent:done".to_string(),
        };
        let existing = HashSet::from(["campaign".to_string(), "agent:ready".to_string()]);
        let statuses = reconcile_managed_labels(&existing, &labels);
        assert!(statuses
            .iter()
            .any(|item| item.name == "campaign" && item.status == "exists"));
        assert!(statuses
            .iter()
            .any(|item| item.name == "night-run" && item.status == "created"));
    }
}
