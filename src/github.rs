use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashSet;
use std::thread;
use std::time::{Duration, Instant};

use crate::{
    agent_exec,
    config::{Config, Labels},
    models::{IssueSnapshot, IssueState},
};

pub struct GitHubClient<'a> {
    config: &'a Config,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CopilotReviewThread {
    pub body: String,
    pub path: Option<String>,
    pub line: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopilotReviewBundle {
    pub pr_number: u64,
    pub head_sha: String,
    pub threads: Vec<CopilotReviewThread>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestLifecycle {
    pub number: u64,
    pub url: String,
    pub state: String,
    pub merged: bool,
    pub draft: bool,
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

    pub fn request_pr_review(&self, pr_target: &str, reviewer: &str) -> Result<()> {
        let command =
            build_request_pr_review_command(&self.config.repo_slug(), pr_target, reviewer);
        let result =
            agent_exec::run_shell_command(&command, &self.config.working_directory(), &[], None)?;
        if !result.success() {
            bail!("gh_pr_review_request_failed");
        }
        Ok(())
    }

    pub fn list_labels(&self) -> Result<Vec<String>> {
        let command = format!(
            "gh label list --repo {} --json name",
            self.config.repo_slug()
        );
        let result =
            agent_exec::run_shell_command(&command, &self.config.working_directory(), &[], None)?;
        if !result.success() {
            bail!("gh_label_list_failed");
        }
        parse_label_list_json(&result.stdout)
    }

    pub fn create_label(&self, name: &str, color: &str, description: &str) -> Result<()> {
        let command =
            build_create_label_command(&self.config.repo_slug(), name, color, description);
        let result =
            agent_exec::run_shell_command(&command, &self.config.working_directory(), &[], None)?;
        if !result.success() {
            bail!("gh_label_create_failed");
        }
        Ok(())
    }

    pub fn setup_labels(&self) -> Result<Vec<ManagedLabelStatus>> {
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

    pub fn ensure_managed_labels(&self) -> Result<Vec<ManagedLabelStatus>> {
        self.setup_labels()
    }

    pub fn normalize_issue_labels(
        &self,
        issue_number: u64,
        remove: &[&str],
        add: &[&str],
    ) -> Result<()> {
        self.remove_labels(issue_number, remove)?;
        self.add_labels(issue_number, add)?;
        Ok(())
    }

    pub fn pr_number_from_url(&self, pr_url: &str) -> Result<u64> {
        let number = pr_url
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .ok_or_else(|| anyhow!("gh_pr_number_parse_failed"))?
            .parse::<u64>()?;
        Ok(number)
    }

    pub fn current_pr_head_sha(&self, pr_number: u64) -> Result<String> {
        let command = format!(
            "gh api repos/{}/pulls/{}",
            self.config.repo_slug(),
            pr_number
        );
        let result =
            agent_exec::run_shell_command(&command, &self.config.working_directory(), &[], None)?;
        if !result.success() {
            bail!("gh_pr_view_failed");
        }
        let parsed = serde_json::from_str::<PullRequestJson>(&result.stdout)
            .context("failed to parse pr json")?;
        Ok(parsed.head.sha)
    }

    pub fn poll_copilot_review(
        &self,
        pr_number: u64,
        interval_seconds: u32,
        timeout_minutes: u32,
    ) -> Result<CopilotReviewBundle> {
        let deadline = Instant::now() + Duration::from_secs((timeout_minutes as u64) * 60);
        loop {
            let bundle = self.fetch_copilot_review_bundle(pr_number)?;
            if !bundle.threads.is_empty() {
                return Ok(bundle);
            }
            if Instant::now() >= deadline {
                bail!("copilot_review_timeout");
            }
            thread::sleep(Duration::from_secs(interval_seconds as u64));
        }
    }

    pub fn fetch_copilot_review_bundle(&self, pr_number: u64) -> Result<CopilotReviewBundle> {
        let head_sha = self.current_pr_head_sha(pr_number)?;
        let reviews_command = format!(
            "gh api repos/{}/pulls/{}/reviews",
            self.config.repo_slug(),
            pr_number
        );
        let comments_command = format!(
            "gh api repos/{}/pulls/{}/comments",
            self.config.repo_slug(),
            pr_number
        );
        let reviews = agent_exec::run_shell_command(
            &reviews_command,
            &self.config.working_directory(),
            &[],
            None,
        )?;
        if !reviews.success() {
            bail!("gh_pr_review_poll_failed");
        }
        let comments = agent_exec::run_shell_command(
            &comments_command,
            &self.config.working_directory(),
            &[],
            None,
        )?;
        if !comments.success() {
            bail!("gh_pr_comments_fetch_failed");
        }
        let review_items = serde_json::from_str::<Vec<PullReviewJson>>(&reviews.stdout)
            .context("failed to parse review json")?;
        let comment_items = serde_json::from_str::<Vec<PullCommentJson>>(&comments.stdout)
            .context("failed to parse comment json")?;

        let mut threads = review_items
            .into_iter()
            .filter(|item| is_copilot_login(&item.user.login) && item.commit_id == head_sha)
            .filter_map(|item| {
                let body = item.body.trim().to_string();
                if body.is_empty() {
                    None
                } else {
                    Some(CopilotReviewThread {
                        body,
                        path: None,
                        line: None,
                    })
                }
            })
            .collect::<Vec<_>>();

        threads.extend(
            comment_items
                .into_iter()
                .filter(|item| is_copilot_login(&item.user.login) && item.commit_id == head_sha)
                .filter_map(|item| {
                    let body = item.body.trim().to_string();
                    if body.is_empty() {
                        None
                    } else {
                        Some(CopilotReviewThread {
                            body,
                            path: item.path,
                            line: item.line,
                        })
                    }
                }),
        );

        Ok(CopilotReviewBundle {
            pr_number,
            head_sha,
            threads,
        })
    }

    pub fn pull_request_lifecycle(&self, pr_number: u64) -> Result<PullRequestLifecycle> {
        let command = format!(
            "gh api repos/{}/pulls/{}",
            self.config.repo_slug(),
            pr_number
        );
        let result =
            agent_exec::run_shell_command(&command, &self.config.working_directory(), &[], None)?;
        if !result.success() {
            bail!("gh_pr_view_failed");
        }
        let parsed = serde_json::from_str::<PullRequestJson>(&result.stdout)
            .context("failed to parse pr json")?;
        Ok(PullRequestLifecycle {
            number: parsed.number,
            url: parsed.html_url,
            state: parsed.state,
            merged: parsed.merged_at.is_some(),
            draft: parsed.draft,
        })
    }

    pub fn pull_request_lifecycle_from_url(&self, pr_url: &str) -> Result<PullRequestLifecycle> {
        let pr_number = self.pr_number_from_url(pr_url)?;
        self.pull_request_lifecycle(pr_number)
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedLabelStatus {
    pub name: String,
    pub status: String,
    pub color: Option<String>,
    pub description: String,
}

#[derive(Debug, Deserialize)]
struct RepoLabelJson {
    name: String,
}

#[derive(Debug, Deserialize)]
struct PullRequestJson {
    number: u64,
    state: String,
    draft: bool,
    html_url: String,
    merged_at: Option<String>,
    head: PullHeadJson,
}

#[derive(Debug, Deserialize)]
struct PullHeadJson {
    sha: String,
}

#[derive(Debug, Deserialize)]
struct PullReviewJson {
    body: String,
    commit_id: String,
    user: PullUserJson,
}

#[derive(Debug, Deserialize)]
struct PullCommentJson {
    body: String,
    commit_id: String,
    path: Option<String>,
    line: Option<u64>,
    user: PullUserJson,
}

#[derive(Debug, Deserialize)]
struct PullUserJson {
    login: String,
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

pub fn build_request_pr_review_command(repo_slug: &str, pr_target: &str, reviewer: &str) -> String {
    format!(
        "gh pr edit {} --repo {} --add-reviewer {}",
        shell_quote(pr_target),
        repo_slug,
        shell_quote(reviewer)
    )
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
    managed_label_specs(labels)
        .into_iter()
        .map(|(name, color, description)| ManagedLabelStatus {
            name: name.to_string(),
            status: if existing.contains(name) {
                "exists".to_string()
            } else {
                "created".to_string()
            },
            color: if existing.contains(name) {
                None
            } else {
                Some(color.to_string())
            },
            description: description.to_string(),
        })
        .collect()
}

fn managed_label_specs(labels: &Labels) -> [(&str, &str, &str); 6] {
    [
        (
            labels.night_run.as_str(),
            "0E8A16",
            "Eligible for nightloop campaign selection",
        ),
        (labels.ready.as_str(), "1D76DB", "Ready for agent execution"),
        (
            labels.running.as_str(),
            "FBCA04",
            "Currently running in nightloop",
        ),
        (labels.review.as_str(), "5319E7", "Ready for human review"),
        (
            labels.blocked.as_str(),
            "B60205",
            "Blocked during nightloop execution",
        ),
        (labels.done.as_str(), "0E8A16", "Completed by nightloop"),
    ]
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn is_copilot_login(login: &str) -> bool {
    login.eq_ignore_ascii_case("github-copilot[bot]")
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crate::config::Labels;

    use super::{
        build_create_label_command, build_request_pr_review_command, parse_issue_json,
        parse_label_list_json, reconcile_managed_labels,
    };

    #[test]
    fn parses_issue_fixture_json() {
        let issue = parse_issue_json(include_str!("../tests/fixtures/github_issue.json")).unwrap();
        assert_eq!(issue.number, 221);
        assert!(issue.has_label("campaign"));
    }

    #[test]
    fn builds_review_request_command() {
        let command = build_request_pr_review_command(
            "o/r",
            "https://github.com/o/r/pull/1",
            "github-copilot[bot]",
        );
        assert_eq!(
            command,
            "gh pr edit 'https://github.com/o/r/pull/1' --repo o/r --add-reviewer 'github-copilot[bot]'"
        );
    }

    #[test]
    fn parses_label_fixture_json() {
        let labels =
            parse_label_list_json(include_str!("../tests/fixtures/github_labels.json")).unwrap();
        assert_eq!(
            labels,
            vec!["night-run".to_string(), "agent:ready".to_string()]
        );
    }

    #[test]
    fn builds_create_label_command() {
        let command = build_create_label_command(
            "o/r",
            "agent:running",
            "FBCA04",
            "Currently running in nightloop",
        );
        assert_eq!(
            command,
            "gh label create 'agent:running' --repo o/r --color 'FBCA04' --description 'Currently running in nightloop'"
        );
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
        let existing = HashSet::from([
            "night-run".to_string(),
            "agent:ready".to_string(),
            "unrelated".to_string(),
        ]);
        let statuses = reconcile_managed_labels(&existing, &labels);
        assert_eq!(statuses.len(), 6);
        assert_eq!(statuses[0].name, "night-run");
        assert_eq!(statuses[0].status, "exists");
        assert_eq!(statuses[0].color, None);
        assert_eq!(statuses[2].name, "agent:running");
        assert_eq!(statuses[2].status, "created");
        assert_eq!(statuses[2].color.as_deref(), Some("FBCA04"));
    }
}
