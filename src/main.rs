use std::{env, path::PathBuf, process};

use anyhow::{anyhow, bail, Result};
use nightloop::{
    budget, config::Config, docs_support, estimate, issue_lint, reporting, runner, telemetry,
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
    Run {
        parent: u64,
        hours: u32,
        dry_run: bool,
    },
    Help,
}

#[derive(Debug)]
struct Cli {
    config_path: PathBuf,
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
    if matches!(cli.command, Command::Help) {
        print_help(None);
        return Ok(());
    }

    let config = Config::load(&cli.config_path)?;

    match cli.command {
        Command::Budget { hours } => {
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
            let record = telemetry::read_run_record(&path)?;
            let history_path = config.telemetry_history_path();
            telemetry::append_run_record(&history_path, &record)?;
            reporting::print_pairs(&[
                ("ok", "true".to_string()),
                ("history_path", history_path.display().to_string()),
            ]);
        }
        Command::DocsCheck => {
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
                bail!("docs-check failed");
            }
        }
        Command::Run {
            parent,
            hours,
            dry_run,
        } => {
            let report = if dry_run {
                runner::dry_run(&config, parent, hours)?
            } else {
                runner::run_campaign(&config, parent, hours)?
            };
            report.print();
            if !report.ok {
                bail!("run failed");
            }
        }
        Command::Help => unreachable!(),
    }

    Ok(())
}

fn parse_cli<I>(args: I) -> Result<Cli>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter().peekable();
    let mut config_path = PathBuf::from("nightloop.toml");

    loop {
        match args.peek().map(String::as_str) {
            Some("--config") => {
                args.next();
                let value = args
                    .next()
                    .ok_or_else(|| anyhow!("--config requires a path"))?;
                config_path = PathBuf::from(value);
            }
            Some("--help") | Some("-h") => {
                args.next();
                return Ok(Cli {
                    config_path,
                    command: Command::Help,
                });
            }
            _ => break,
        }
    }

    let Some(command_name) = args.next() else {
        return Ok(Cli {
            config_path,
            command: Command::Help,
        });
    };

    let command = match command_name.as_str() {
        "budget" => parse_budget(args.collect())?,
        "lint-issue" => parse_lint_issue(args.collect())?,
        "estimate-issue" => parse_estimate_issue(args.collect())?,
        "record-run" => parse_record_run(args.collect())?,
        "docs-check" => parse_docs_check(args.collect())?,
        "run" => parse_run(args.collect())?,
        "help" => Command::Help,
        other => bail!("unknown command: {other}"),
    };

    Ok(Cli {
        config_path,
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
        print_help(Some("lint-issue"));
        process::exit(0);
    }
    if args.len() != 1 {
        bail!("lint-issue requires exactly one path");
    }
    Ok(Command::LintIssue {
        path: PathBuf::from(&args[0]),
    })
}

fn parse_estimate_issue(args: Vec<String>) -> Result<Command> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help(Some("estimate-issue"));
        process::exit(0);
    }
    if args.is_empty() {
        bail!("estimate-issue requires a path");
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
            other => bail!("unexpected argument for estimate-issue: {other}"),
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
        print_help(Some("docs-check"));
        process::exit(0);
    }
    if !args.is_empty() {
        bail!("docs-check does not accept additional arguments");
    }
    Ok(Command::DocsCheck)
}

fn parse_run(args: Vec<String>) -> Result<Command> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help(Some("run"));
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
            other => bail!("unexpected argument for run: {other}"),
        }
    }
    Ok(Command::Run {
        parent: parent.ok_or_else(|| anyhow!("run requires --parent"))?,
        hours: hours.ok_or_else(|| anyhow!("run requires --hours"))?,
        dry_run,
    })
}

fn print_help(command: Option<&str>) {
    let text = match command {
        Some("budget") => {
            "Usage: nightloop [--config PATH] budget --hours 2|3|4|5|6\n\
\n\
Compute the fallback slot count for a night window.\n"
        }
        Some("lint-issue") => {
            "Usage: nightloop [--config PATH] lint-issue path/to/issue.md\n\
\n\
Validate a child issue markdown snapshot.\n"
        }
        Some("estimate-issue") => {
            "Usage: nightloop [--config PATH] estimate-issue path/to/issue.md [--basis template|local|hybrid|ai]\n\
\n\
Estimate model selection and runtime for a child issue.\n"
        }
        Some("record-run") => {
            "Usage: nightloop [--config PATH] record-run path/to/run-record.json\n\
\n\
Append a run record to local telemetry.\n"
        }
        Some("docs-check") => {
            "Usage: nightloop [--config PATH] docs-check\n\
\n\
Validate required docs, templates, and prompt files.\n"
        }
        Some("run") => {
            "Usage: nightloop [--config PATH] run --parent ISSUE --hours 2|3|4|5|6 [--dry-run]\n\
\n\
Execute or simulate a parent issue campaign.\n"
        }
        _ => {
            "nightloop\n\
\n\
Issue-first nightly runner for coding agents.\n\
\n\
Usage:\n\
  nightloop [--config PATH] budget --hours 2|3|4|5|6\n\
  nightloop [--config PATH] lint-issue path/to/issue.md\n\
  nightloop [--config PATH] estimate-issue path/to/issue.md [--basis template|local|hybrid|ai]\n\
  nightloop [--config PATH] record-run path/to/run-record.json\n\
  nightloop [--config PATH] docs-check\n\
  nightloop [--config PATH] run --parent ISSUE --hours 2|3|4|5|6 [--dry-run]\n\
\n\
Global options:\n\
  --config PATH   Path to nightloop.toml (default: nightloop.toml)\n\
  --help          Show this help output\n"
        }
    };
    println!("{text}");
}
