use std::{env, path::PathBuf, process};

use anyhow::{anyhow, bail, Result};
use nightloop::{
    budget,
    config::{self, Config},
    docs_support, estimate, issue_lint, reporting, runner, telemetry,
};

#[derive(Debug)]
enum Command {
    Budget {
        hours: u32,
    },
    LintIssue {
        path: PathBuf,
    },
    EstimateIssue {
        path: PathBuf,
        basis: estimate::EstimateBasis,
    },
    RecordRun {
        path: PathBuf,
    },
    DocsCheck,
    SetupLabels,
    InitTarget {
        name: String,
        repo: String,
        workdir: PathBuf,
        base_branch: String,
        agent_command: String,
        plan_command: String,
        default_model: String,
        default_reasoning_effort: String,
        request_copilot_review: bool,
    },
    Run {
        parent: u64,
        hours: u32,
        dry_run: bool,
    },
    ReviewLoop {
        parent: u64,
        dry_run: bool,
    },
    Help,
}

#[derive(Debug)]
struct Cli {
    explicit_config_path: Option<PathBuf>,
    target_name: Option<String>,
    verbose: bool,
    command: Command,
}

fn main() {
    if let Err(err) = real_main() {
        eprintln!(
            "ok=false error={}",
            reporting::escape_value(&err.to_string())
        );
        process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let cli = parse_cli(env::args().skip(1))?;
    nightloop::agent_exec::set_verbose_commands(cli.verbose);
    if matches!(cli.command, Command::Help) {
        print_help(None);
        return Ok(());
    }
    let cwd = env::current_dir()?;
    let config = if matches!(cli.command, Command::InitTarget { .. } | Command::Help) {
        None
    } else {
        Some(load_config_for_cli(&cwd, &cli)?)
    };

    match cli.command {
        Command::InitTarget {
            name,
            repo,
            workdir,
            base_branch,
            agent_command,
            plan_command,
            default_model,
            default_reasoning_effort,
            request_copilot_review,
        } => {
            let control_root =
                config::resolve_control_root(&cwd, cli.explicit_config_path.as_deref());
            let (owner, repo_name) = repo
                .split_once('/')
                .ok_or_else(|| anyhow!("--repo must be OWNER/REPO"))?;
            let template_path = control_root.join("nightloop.example.toml");
            let template = std::fs::read_to_string(&template_path)
                .map_err(|_| anyhow!("failed to read template from {}", template_path.display()))?;
            let rendered = config::render_named_target_config(
                &template,
                owner,
                repo_name,
                &workdir,
                &base_branch,
                &agent_command,
                &plan_command,
                &default_model,
                &default_reasoning_effort,
                request_copilot_review,
            );
            let target_dir = control_root.join("targets");
            std::fs::create_dir_all(&target_dir)?;
            let target_path = target_dir.join(format!("{name}.toml"));
            if target_path.exists() {
                bail!("target_config_exists");
            }
            std::fs::write(&target_path, rendered)?;
            reporting::print_pairs(&[
                ("ok", "true".to_string()),
                ("target", name),
                ("config_path", target_path.display().to_string()),
                ("agent_command", agent_command),
                ("plan_command", plan_command),
                ("default_model", default_model),
                ("default_reasoning_effort", default_reasoning_effort),
                ("request_copilot_review", request_copilot_review.to_string()),
            ]);
        }
        Command::Budget { hours } => {
            let config = config.as_ref().unwrap();
            let report = budget::budget_report(
                hours,
                config.loop_cfg.fallback_cycle_minutes,
                config.loop_cfg.fixed_overhead_minutes,
                config.loop_cfg.min_hours,
                config.loop_cfg.max_hours,
            )?;
            reporting::print_pairs(&[
                ("hours", report.hours.to_string()),
                ("available_minutes", report.available_minutes.to_string()),
                (
                    "fixed_overhead_minutes",
                    report.fixed_overhead_minutes.to_string(),
                ),
                (
                    "fallback_cycle_minutes",
                    report.fallback_cycle_minutes.to_string(),
                ),
                ("fallback_slots", report.fallback_slots.to_string()),
            ]);
        }
        Command::LintIssue { path } => {
            let config = config.as_ref().unwrap();
            let report = issue_lint::lint_markdown_issue(&config, &path)?;
            reporting::print_pairs(&[
                ("valid", report.valid.to_string()),
                ("issue_kind", "child".to_string()),
                ("error_count", report.findings.len().to_string()),
            ]);
            for finding in &report.findings {
                reporting::print_pairs(&[
                    ("finding", finding.code.clone()),
                    ("field", finding.field.clone().unwrap_or_default()),
                    ("message", finding.message.clone()),
                ]);
            }
            if !report.valid {
                bail!("lint failed");
            }
        }
        Command::EstimateIssue { path, basis } => {
            let config = config.as_ref().unwrap();
            let lint = issue_lint::lint_markdown_issue(&config, &path)?;
            if !lint.valid {
                reporting::print_pairs(&[
                    ("valid", "false".to_string()),
                    ("error_count", lint.findings.len().to_string()),
                ]);
                for finding in &lint.findings {
                    reporting::print_pairs(&[
                        ("finding", finding.code.clone()),
                        ("field", finding.field.clone().unwrap_or_default()),
                        ("message", finding.message.clone()),
                    ]);
                }
                bail!("estimate requires a valid child issue");
            }
            let child = lint
                .child
                .ok_or_else(|| anyhow!("missing parsed child issue after successful lint"))?;
            let report = estimate::estimate_child_issue(&config, &child, basis)?;
            reporting::print_pairs(&[
                ("model_profile", report.model_profile.clone()),
                ("model", report.model.clone()),
                ("reasoning_effort", report.reasoning_effort.clone()),
                ("estimated_minutes", report.estimated_minutes.to_string()),
                (
                    "recommended_single_issue_window_hours",
                    report.recommended_hours.to_string(),
                ),
                ("basis_requested", report.basis_requested.clone()),
                ("basis_used", report.basis_used.clone()),
                ("local_samples", report.local_samples.to_string()),
            ]);
            if let Some(ai) = &report.ai_estimate {
                reporting::print_pairs(&[
                    ("ai_model_profile", ai.model_profile.clone()),
                    ("ai_estimated_minutes", ai.estimated_minutes.to_string()),
                    ("ai_confidence", ai.confidence.as_str().to_string()),
                    ("ai_notes", ai.notes.clone()),
                ]);
            }
            for note in &report.notes {
                reporting::print_pairs(&[("note", note.clone())]);
            }
        }
        Command::RecordRun { path } => {
            let config = config.as_ref().unwrap();
            let record = telemetry::read_run_record(&path)?;
            let history_path = config.telemetry_history_path();
            telemetry::append_run_record(&history_path, &record)?;
            reporting::print_pairs(&[
                ("ok", "true".to_string()),
                ("history_path", history_path.display().to_string()),
            ]);
        }
        Command::DocsCheck => {
            let config = config.as_ref().unwrap();
            let report = docs_support::check_docs(&config)?;
            reporting::print_pairs(&[
                ("ok", report.ok.to_string()),
                ("missing_count", report.missing_paths.len().to_string()),
            ]);
            for missing in &report.missing_paths {
                reporting::print_pairs(&[
                    ("missing_kind", missing.kind.clone()),
                    ("missing_path", missing.path.display().to_string()),
                ]);
            }
            if !report.ok {
                bail!("check failed");
            }
        }
        Command::SetupLabels => {
            let config = config.as_ref().unwrap();
            let github = nightloop::github::GitHubClient::new(config);
            github.check_auth()?;
            let statuses = github.setup_labels()?;
            let created_count = statuses
                .iter()
                .filter(|item| item.status == "created")
                .count();
            let existing_count = statuses
                .iter()
                .filter(|item| item.status == "exists")
                .count();
            reporting::print_pairs(&[
                ("ok", "true".to_string()),
                ("repo", config.repo_slug()),
                ("created_count", created_count.to_string()),
                ("existing_count", existing_count.to_string()),
            ]);
            for item in statuses {
                reporting::print_pairs(&[("label", item.name), ("status", item.status)]);
            }
        }
        Command::Run {
            parent,
            hours,
            dry_run,
        } => {
            let config = config.as_ref().unwrap();
            let report = if dry_run {
                runner::dry_run(&config, parent, hours)?
            } else {
                runner::run_campaign(&config, parent, hours)?
            };
            report.print();
            if !report.ok {
                bail!("nightly failed");
            }
        }
        Command::ReviewLoop { parent, dry_run } => {
            let config = config.as_ref().unwrap();
            let report = if dry_run {
                runner::review_loop_dry_run(&config, parent)?
            } else {
                runner::review_loop(&config, parent)?
            };
            report.print();
            if !report.ok {
                bail!("start failed");
            }
        }
        Command::Help => unreachable!(),
    }

    Ok(())
}

fn load_config_for_cli(cwd: &std::path::Path, cli: &Cli) -> Result<Config> {
    let config_path = config::resolve_config_path(
        cwd,
        cli.explicit_config_path.as_deref(),
        cli.target_name.as_deref(),
    )?;
    if cli.explicit_config_path.is_none() && cli.target_name.is_some() {
        Config::load_with_control_root(&config_path, cwd)
    } else {
        Config::load(&config_path)
    }
}

fn parse_cli<I>(args: I) -> Result<Cli>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    let mut explicit_config_path = None;
    let mut target_name = None;
    let mut verbose = false;
    let mut rest = Vec::new();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow!("--config requires a path"))?;
                explicit_config_path = Some(PathBuf::from(value));
            }
            "--target" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow!("--target requires a name"))?;
                target_name = Some(value);
            }
            "--verbose" => {
                verbose = true;
            }
            _ => rest.push(arg),
        }
    }

    if rest.len() == 1 && matches!(rest.first().map(String::as_str), Some("--help" | "-h")) {
        return Ok(Cli {
            explicit_config_path,
            target_name,
            verbose,
            command: Command::Help,
        });
    }

    let Some(command_name) = rest.first() else {
        return Ok(Cli {
            explicit_config_path,
            target_name,
            verbose,
            command: Command::Help,
        });
    };

    let command = match command_name.as_str() {
        "budget" => parse_budget(rest[1..].to_vec())?,
        "lint" | "lint-issue" => parse_lint_issue(rest[1..].to_vec())?,
        "estimate" | "estimate-issue" => parse_estimate_issue(rest[1..].to_vec())?,
        "record-run" => parse_record_run(rest[1..].to_vec())?,
        "check" | "docs-check" => parse_docs_check(rest[1..].to_vec())?,
        "setup-labels" => parse_setup_labels(rest[1..].to_vec())?,
        "init" | "init-target" => parse_init_target(rest[1..].to_vec())?,
        "nightly" | "run" => parse_run(rest[1..].to_vec())?,
        "start" | "review-loop" => parse_review_loop(rest[1..].to_vec())?,
        "help" => Command::Help,
        other => bail!("unknown command: {other}"),
    };

    Ok(Cli {
        explicit_config_path,
        target_name,
        verbose,
        command,
    })
}

fn parse_budget(args: Vec<String>) -> Result<Command> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help(Some("budget"));
        process::exit(0);
    }
    let mut hours = None;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--hours" => {
                let value = iter
                    .next()
                    .ok_or_else(|| anyhow!("--hours requires a value"))?;
                hours = Some(value.parse::<u32>()?);
            }
            other => bail!("unexpected argument for budget: {other}"),
        }
    }
    Ok(Command::Budget {
        hours: hours.ok_or_else(|| anyhow!("budget requires --hours"))?,
    })
}

fn parse_lint_issue(args: Vec<String>) -> Result<Command> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help(Some("lint"));
        process::exit(0);
    }
    if args.len() != 1 {
        bail!("lint requires exactly one path");
    }
    Ok(Command::LintIssue {
        path: PathBuf::from(&args[0]),
    })
}

fn parse_estimate_issue(args: Vec<String>) -> Result<Command> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help(Some("estimate"));
        process::exit(0);
    }
    if args.is_empty() {
        bail!("estimate requires a path");
    }
    let path = PathBuf::from(&args[0]);
    let mut basis = estimate::EstimateBasis::Hybrid;
    let mut iter = args.into_iter().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--basis" => {
                let value = iter
                    .next()
                    .ok_or_else(|| anyhow!("--basis requires a value"))?;
                basis = estimate::EstimateBasis::from_cli_str(&value)?;
            }
            other => bail!("unexpected argument for estimate: {other}"),
        }
    }
    Ok(Command::EstimateIssue { path, basis })
}

fn parse_record_run(args: Vec<String>) -> Result<Command> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help(Some("record-run"));
        process::exit(0);
    }
    if args.len() != 1 {
        bail!("record-run requires exactly one path");
    }
    Ok(Command::RecordRun {
        path: PathBuf::from(&args[0]),
    })
}

fn parse_docs_check(args: Vec<String>) -> Result<Command> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help(Some("check"));
        process::exit(0);
    }
    if !args.is_empty() {
        bail!("check does not accept additional arguments");
    }
    Ok(Command::DocsCheck)
}

fn parse_setup_labels(args: Vec<String>) -> Result<Command> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help(Some("setup-labels"));
        process::exit(0);
    }
    if !args.is_empty() {
        bail!("setup-labels does not accept additional arguments");
    }
    Ok(Command::SetupLabels)
}

fn parse_init_target(args: Vec<String>) -> Result<Command> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help(Some("init"));
        process::exit(0);
    }
    let mut name = None;
    let mut repo = None;
    let mut workdir = None;
    let mut base_branch = "main".to_string();
    let mut agent_command = "codex exec --full-auto".to_string();
    let mut plan_command = "codex exec --full-auto".to_string();
    let mut default_model = "gpt-5.4".to_string();
    let mut default_reasoning_effort = "medium".to_string();
    let mut request_copilot_review = false;
    let mut positional = Vec::new();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--name" => {
                name = Some(
                    iter.next()
                        .ok_or_else(|| anyhow!("--name requires a value"))?,
                );
            }
            "--repo" => {
                repo = Some(
                    iter.next()
                        .ok_or_else(|| anyhow!("--repo requires a value"))?,
                );
            }
            "--workdir" => {
                workdir = Some(PathBuf::from(
                    iter.next()
                        .ok_or_else(|| anyhow!("--workdir requires a value"))?,
                ));
            }
            "--base-branch" => {
                base_branch = iter
                    .next()
                    .ok_or_else(|| anyhow!("--base-branch requires a value"))?;
            }
            "--agent-command" => {
                agent_command = iter
                    .next()
                    .ok_or_else(|| anyhow!("--agent-command requires a value"))?;
            }
            "--plan-command" => {
                plan_command = iter
                    .next()
                    .ok_or_else(|| anyhow!("--plan-command requires a value"))?;
            }
            "--default-model" => {
                default_model = iter
                    .next()
                    .ok_or_else(|| anyhow!("--default-model requires a value"))?;
            }
            "--default-reasoning-effort" => {
                default_reasoning_effort = iter
                    .next()
                    .ok_or_else(|| anyhow!("--default-reasoning-effort requires a value"))?;
            }
            "--request-copilot-review" => {
                request_copilot_review = true;
            }
            other if other.starts_with("--") => bail!("unexpected argument for init: {other}"),
            other => positional.push(other.to_string()),
        }
    }
    if positional.len() > 3 {
        bail!("init accepts at most NAME OWNER/REPO WORKDIR as positional arguments");
    }
    if name.is_none() {
        name = positional.first().cloned();
    }
    if repo.is_none() {
        repo = positional.get(1).cloned();
    }
    if workdir.is_none() {
        workdir = positional.get(2).map(PathBuf::from);
    }
    Ok(Command::InitTarget {
        name: name.ok_or_else(|| anyhow!("init requires NAME or --name"))?,
        repo: repo.ok_or_else(|| anyhow!("init requires OWNER/REPO or --repo"))?,
        workdir: workdir.ok_or_else(|| anyhow!("init requires WORKDIR or --workdir"))?,
        base_branch,
        agent_command,
        plan_command,
        default_model,
        default_reasoning_effort,
        request_copilot_review,
    })
}

fn parse_review_loop(args: Vec<String>) -> Result<Command> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help(Some("start"));
        process::exit(0);
    }
    let mut parent = None;
    let mut dry_run = false;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--parent" => {
                let value = iter
                    .next()
                    .ok_or_else(|| anyhow!("--parent requires a value"))?;
                parent = Some(value.parse::<u64>()?);
            }
            "--dry-run" => dry_run = true,
            other if other.starts_with("--") => bail!("unexpected argument for start: {other}"),
            other => {
                if parent.is_some() {
                    bail!("start accepts only one parent issue");
                }
                parent = Some(other.parse::<u64>()?);
            }
        }
    }
    Ok(Command::ReviewLoop {
        parent: parent.ok_or_else(|| anyhow!("start requires PARENT_ISSUE"))?,
        dry_run,
    })
}

fn parse_run(args: Vec<String>) -> Result<Command> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help(Some("nightly"));
        process::exit(0);
    }
    let mut parent = None;
    let mut hours = None;
    let mut dry_run = false;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--parent" => {
                let value = iter
                    .next()
                    .ok_or_else(|| anyhow!("--parent requires a value"))?;
                parent = Some(value.parse::<u64>()?);
            }
            "--hours" => {
                let value = iter
                    .next()
                    .ok_or_else(|| anyhow!("--hours requires a value"))?;
                hours = Some(value.parse::<u32>()?);
            }
            "--dry-run" => dry_run = true,
            other if other.starts_with("--") => bail!("unexpected argument for nightly: {other}"),
            other => {
                if parent.is_some() {
                    bail!("nightly accepts only one parent issue");
                }
                parent = Some(other.parse::<u64>()?);
            }
        }
    }
    Ok(Command::Run {
        parent: parent.ok_or_else(|| anyhow!("nightly requires PARENT_ISSUE"))?,
        hours: hours.ok_or_else(|| anyhow!("nightly requires --hours"))?,
        dry_run,
    })
}

fn print_help(command: Option<&str>) {
    let text = match command {
        Some("check") | Some("docs-check") => {
            "Usage: nightloop check [--target NAME]\n\
\n\
Global options: --verbose\n\
\n\
Validate required docs, templates, and prompt files.\n\
\n\
Compatibility alias: docs-check\n"
        }
        Some("lint") | Some("lint-issue") => {
            "Usage: nightloop lint path/to/issue.md [--target NAME]\n\
\n\
Global options: --verbose\n\
\n\
Validate a child issue markdown snapshot.\n\
\n\
Compatibility alias: lint-issue\n"
        }
        Some("estimate") | Some("estimate-issue") => {
            "Usage: nightloop estimate path/to/issue.md [--target NAME] [--basis template|local|hybrid|ai]\n\
\n\
Global options: --verbose\n\
\n\
Estimate model selection and runtime for a child issue.\n\
\n\
Compatibility alias: estimate-issue\n"
        }
        Some("init") | Some("init-target") => {
            "Usage: nightloop init NAME OWNER/REPO WORKDIR [--base-branch main] [--agent-command CMD] [--plan-command CMD] [--default-model MODEL] [--default-reasoning-effort LEVEL] [--request-copilot-review]\n\
\n\
Global options: --verbose\n\
\n\
Create targets/NAME.toml from the example template and fill common initial settings.\n\
\n\
Compatibility alias: init-target --name NAME --repo OWNER/REPO --workdir PATH\n"
        }
        Some("nightly") | Some("run") => {
            "Usage: nightloop nightly PARENT_ISSUE [--target NAME] --hours 2|3|4|5|6 [--dry-run]\n\
\n\
Global options: --verbose\n\
\n\
Execute or simulate the budget-based nightly campaign.\n\
\n\
Compatibility alias: run --parent ISSUE --hours 2|3|4|5|6 [--dry-run]\n"
        }
        Some("start") | Some("review-loop") => {
            "Usage: nightloop start PARENT_ISSUE [--target NAME] [--dry-run]\n\
\n\
Global options: --verbose\n\
\n\
Plan, implement, request Copilot review, wait, and apply one fix round for the first runnable child issue.\n\
\n\
Compatibility alias: review-loop --parent ISSUE [--dry-run]\n"
        }
        Some("budget") => {
            "Usage: nightloop [--config PATH] [--target NAME] budget --hours 2|3|4|5|6\n\
\n\
Global options: --verbose\n\
\n\
Compute the fallback slot count for a night window.\n"
        }
        Some("record-run") => {
            "Usage: nightloop [--config PATH] [--target NAME] record-run path/to/run-record.json\n\
\n\
Global options: --verbose\n\
\n\
Append a run record to local telemetry.\n"
        }
        Some("setup-labels") => {
            "Usage: nightloop setup-labels [--target NAME]\n\
\n\
Global options: --verbose\n\
\n\
Create any missing workflow labels required by nightloop.\n"
        }
        _ => {
            "nightloop\n\
\n\
Issue-first nightly runner for coding agents.\n\
\n\
Usage:\n\
  nightloop init NAME OWNER/REPO WORKDIR [--agent-command CMD]\n\
  nightloop check [--target NAME]\n\
  nightloop lint path/to/issue.md [--target NAME]\n\
  nightloop estimate path/to/issue.md [--target NAME] [--basis template|local|hybrid|ai]\n\
  nightloop start PARENT_ISSUE [--target NAME] [--dry-run]\n\
  nightloop nightly PARENT_ISSUE [--target NAME] --hours 2|3|4|5|6 [--dry-run]\n\
  nightloop setup-labels [--target NAME]\n\
  nightloop [--config PATH] budget --hours 2|3|4|5|6\n\
  nightloop [--config PATH] record-run path/to/run-record.json\n\
\n\
Compatibility aliases:\n\
  init-target, docs-check, lint-issue, estimate-issue, review-loop, run\n\
\n\
Global options:\n\
  --config PATH   Explicit config path; overrides --target\n\
  --target NAME   Load targets/NAME.toml from the control repo\n\
  --verbose       Stream executed commands and live subprocess output to stderr\n\
  --help          Show this help output\n"
        }
    };
    println!("{text}");
}

#[cfg(test)]
mod tests {
    use super::{parse_cli, Cli, Command};

    fn parse(args: &[&str]) -> Cli {
        parse_cli(args.iter().map(|arg| arg.to_string())).unwrap()
    }

    #[test]
    fn start_accepts_positional_parent_and_legacy_flag() {
        match parse(&["start", "221", "--dry-run"]).command {
            Command::ReviewLoop { parent, dry_run } => {
                assert_eq!(parent, 221);
                assert!(dry_run);
            }
            other => panic!("unexpected command: {other:?}"),
        }

        match parse(&["review-loop", "--parent", "222"]).command {
            Command::ReviewLoop { parent, dry_run } => {
                assert_eq!(parent, 222);
                assert!(!dry_run);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn nightly_accepts_positional_parent_and_legacy_flag() {
        match parse(&["nightly", "221", "--hours", "4"]).command {
            Command::Run {
                parent,
                hours,
                dry_run,
            } => {
                assert_eq!(parent, 221);
                assert_eq!(hours, 4);
                assert!(!dry_run);
            }
            other => panic!("unexpected command: {other:?}"),
        }

        match parse(&["run", "--parent", "222", "--hours", "2", "--dry-run"]).command {
            Command::Run {
                parent,
                hours,
                dry_run,
            } => {
                assert_eq!(parent, 222);
                assert_eq!(hours, 2);
                assert!(dry_run);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn init_accepts_positional_form() {
        match parse(&[
            "init",
            "canaria",
            "UTAGEDA/canaria",
            "/tmp/canaria",
            "--request-copilot-review",
        ])
        .command
        {
            Command::InitTarget {
                name,
                repo,
                workdir,
                agent_command,
                plan_command,
                request_copilot_review,
                ..
            } => {
                assert_eq!(name, "canaria");
                assert_eq!(repo, "UTAGEDA/canaria");
                assert_eq!(workdir.to_string_lossy(), "/tmp/canaria");
                assert_eq!(agent_command, "codex exec --full-auto");
                assert_eq!(plan_command, "codex exec --full-auto");
                assert!(request_copilot_review);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn verbose_is_parsed_as_global_option() {
        let cli = parse(&["--verbose", "start", "221"]);
        assert!(cli.verbose);
        match cli.command {
            Command::ReviewLoop { parent, dry_run } => {
                assert_eq!(parent, 221);
                assert!(!dry_run);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }
}
