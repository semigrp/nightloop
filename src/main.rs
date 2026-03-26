use std::{env, path::PathBuf, process};

use anyhow::{anyhow, bail, Result};
use nightloop::{
    config::{self, Config},
    docs_support, estimate,
    github::GitHubClient,
    issue_lint, reporting, runner,
};

#[derive(Debug)]
enum Command {
    Check,
    Lint {
        path: PathBuf,
    },
    Estimate {
        path: PathBuf,
        basis: estimate::EstimateBasis,
    },
    Init {
        name: String,
        repo: String,
        workdir: PathBuf,
        base_branch: String,
        agent_command: String,
        plan_command: String,
        default_model: String,
        default_reasoning_effort: String,
    },
    Start {
        parent: u64,
        dry_run: bool,
    },
    Nightly {
        parent: u64,
        hours: u32,
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
        print_help();
        return Ok(());
    }

    let cwd = env::current_dir()?;
    let config = if matches!(cli.command, Command::Init { .. }) {
        None
    } else {
        Some(load_config_for_cli(&cwd, &cli)?)
    };

    match cli.command {
        Command::Check => {
            let config = config.as_ref().unwrap();
            let report = docs_support::check_docs(config)?;
            let github = GitHubClient::new(config);
            github.check_auth()?;
            let label_statuses = github.ensure_managed_labels()?;
            let created_count = label_statuses
                .iter()
                .filter(|item| item.status == "created")
                .count();
            let existing_count = label_statuses
                .iter()
                .filter(|item| item.status == "exists")
                .count();
            reporting::print_pairs(&[
                ("ok", report.ok.to_string()),
                ("missing_count", report.missing_paths.len().to_string()),
                ("labels_created", created_count.to_string()),
                ("labels_existing", existing_count.to_string()),
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
        Command::Lint { path } => {
            let config = config.as_ref().unwrap();
            let report = issue_lint::lint_markdown_issue(config, &path)?;
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
        Command::Estimate { path, basis } => {
            let config = config.as_ref().unwrap();
            let lint = issue_lint::lint_markdown_issue(config, &path)?;
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
            let report = estimate::estimate_child_issue(config, &child, basis)?;
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
        }
        Command::Init {
            name,
            repo,
            workdir,
            base_branch,
            agent_command,
            plan_command,
            default_model,
            default_reasoning_effort,
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
            ]);
        }
        Command::Start { parent, dry_run } => {
            let config = config.as_ref().unwrap();
            let report = if dry_run {
                runner::start_dry_run(config, parent)?
            } else {
                runner::start(config, parent)?
            };
            report.print();
            if !report.ok {
                bail!("start failed");
            }
        }
        Command::Nightly {
            parent,
            hours,
            dry_run,
        } => {
            let config = config.as_ref().unwrap();
            let report = if dry_run {
                runner::dry_run(config, parent, hours)?
            } else {
                runner::run_campaign(config, parent, hours)?
            };
            report.print();
            if !report.ok {
                bail!("nightly failed");
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
    let control_root = config::resolve_control_root(cwd, cli.explicit_config_path.as_deref());
    Config::load_with_control_root(&config_path, &control_root)
}

fn parse_cli(args: impl IntoIterator<Item = String>) -> Result<Cli> {
    let mut explicit_config_path = None;
    let mut target_name = None;
    let mut verbose = false;
    let mut rest = Vec::new();

    let mut args = args.into_iter().peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => {
                explicit_config_path = Some(PathBuf::from(next_value(&mut args, "--config")?))
            }
            "--target" => target_name = Some(next_value(&mut args, "--target")?),
            "--verbose" => verbose = true,
            "--help" | "-h" => {
                return Ok(Cli {
                    explicit_config_path,
                    target_name,
                    verbose,
                    command: Command::Help,
                })
            }
            _ => rest.push(arg),
        }
    }

    let command = match rest.first().map(String::as_str) {
        None => Command::Help,
        Some("check") => Command::Check,
        Some("lint") => parse_lint(rest[1..].to_vec())?,
        Some("estimate") => parse_estimate(rest[1..].to_vec())?,
        Some("init") => parse_init(rest[1..].to_vec())?,
        Some("start") => parse_start(rest[1..].to_vec())?,
        Some("nightly") => parse_nightly(rest[1..].to_vec())?,
        Some(other) => bail!("unknown command: {other}"),
    };

    Ok(Cli {
        explicit_config_path,
        target_name,
        verbose,
        command,
    })
}

fn parse_lint(args: Vec<String>) -> Result<Command> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        process::exit(0);
    }
    if args.len() != 1 {
        bail!("lint requires a path");
    }
    Ok(Command::Lint {
        path: PathBuf::from(&args[0]),
    })
}

fn parse_estimate(args: Vec<String>) -> Result<Command> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        process::exit(0);
    }
    if args.is_empty() {
        bail!("estimate requires a path");
    }
    let mut path = None;
    let mut basis = estimate::EstimateBasis::Hybrid;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--basis" => {
                basis = estimate::EstimateBasis::from_cli_str(
                    &args
                        .next()
                        .ok_or_else(|| anyhow!("missing value for --basis"))?,
                )?;
            }
            _ if path.is_none() => path = Some(PathBuf::from(arg)),
            other => bail!("unexpected argument: {other}"),
        }
    }
    Ok(Command::Estimate {
        path: path.ok_or_else(|| anyhow!("estimate requires a path"))?,
        basis,
    })
}

fn parse_init(args: Vec<String>) -> Result<Command> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        process::exit(0);
    }
    let mut positional = Vec::new();
    let mut base_branch = "main".to_string();
    let mut agent_command = "codex exec --full-auto".to_string();
    let mut plan_command = "codex exec --full-auto".to_string();
    let mut default_model = "gpt-5.4".to_string();
    let mut default_reasoning_effort = "medium".to_string();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--base-branch" => base_branch = next_iter_value(&mut args, "--base-branch")?,
            "--agent-command" => agent_command = next_iter_value(&mut args, "--agent-command")?,
            "--plan-command" => plan_command = next_iter_value(&mut args, "--plan-command")?,
            "--default-model" => default_model = next_iter_value(&mut args, "--default-model")?,
            "--default-reasoning-effort" => {
                default_reasoning_effort = next_iter_value(&mut args, "--default-reasoning-effort")?
            }
            _ => positional.push(arg),
        }
    }
    if positional.len() != 3 {
        bail!("init requires NAME OWNER/REPO WORKDIR");
    }
    Ok(Command::Init {
        name: positional.remove(0),
        repo: positional.remove(0),
        workdir: PathBuf::from(positional.remove(0)),
        base_branch,
        agent_command,
        plan_command,
        default_model,
        default_reasoning_effort,
    })
}

fn parse_start(args: Vec<String>) -> Result<Command> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        process::exit(0);
    }
    let mut parent = None;
    let mut dry_run = false;
    for arg in args {
        match arg.as_str() {
            "--dry-run" => dry_run = true,
            _ if parent.is_none() => parent = Some(arg.parse::<u64>()?),
            other => bail!("unexpected argument: {other}"),
        }
    }
    Ok(Command::Start {
        parent: parent.ok_or_else(|| anyhow!("start requires a parent issue number"))?,
        dry_run,
    })
}

fn parse_nightly(args: Vec<String>) -> Result<Command> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        process::exit(0);
    }
    let mut parent = None;
    let mut hours = None;
    let mut dry_run = false;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--hours" => hours = Some(next_iter_value(&mut args, "--hours")?.parse::<u32>()?),
            "--dry-run" => dry_run = true,
            _ if parent.is_none() => parent = Some(arg.parse::<u64>()?),
            other => bail!("unexpected argument: {other}"),
        }
    }
    Ok(Command::Nightly {
        parent: parent.ok_or_else(|| anyhow!("nightly requires a parent issue number"))?,
        hours: hours.ok_or_else(|| anyhow!("nightly requires --hours"))?,
        dry_run,
    })
}

fn next_value(
    args: &mut std::iter::Peekable<impl Iterator<Item = String>>,
    flag: &str,
) -> Result<String> {
    args.next()
        .ok_or_else(|| anyhow!("missing value for {flag}"))
}

fn next_iter_value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String> {
    args.next()
        .ok_or_else(|| anyhow!("missing value for {flag}"))
}

fn print_help() {
    println!(
        "\
Usage:
  nightloop init NAME OWNER/REPO WORKDIR [--base-branch BRANCH] [--agent-command CMD] [--plan-command CMD]
  nightloop check [--target NAME] [--verbose]
  nightloop lint PATH [--target NAME] [--verbose]
  nightloop estimate PATH --basis template|local|hybrid [--target NAME] [--verbose]
  nightloop start PARENT_ISSUE [--target NAME] [--dry-run] [--verbose]
  nightloop nightly PARENT_ISSUE --hours 2|3|4|5|6 [--target NAME] [--dry-run] [--verbose]

Notes:
  --config PATH overrides --target NAME.
  --verbose streams executed commands and live subprocess output to stderr.
"
    );
}
