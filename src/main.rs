mod budget;
mod config;
mod docs_support;
mod errors;
mod estimate;
mod issue_lint;
mod models;
mod selection;
mod telemetry;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use config::Config;
use estimate::EstimateBasis;

#[derive(Parser, Debug)]
#[command(name = "nightloop")]
#[command(about = "Issue-first nightly runner for coding agents")]
struct Cli {
    #[arg(long, default_value = "nightloop.toml")]
    config: PathBuf,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Compute how many child Issues fit in the selected time budget using the legacy fallback cycle.
    Budget {
        #[arg(long)]
        hours: u32,
    },

    /// Validate a markdown Issue snapshot against the required structure.
    LintIssue {
        path: PathBuf,
    },

    /// Estimate model choice and runtime for a child Issue.
    EstimateIssue {
        path: PathBuf,
        #[arg(long, value_enum, default_value_t = EstimateBasisArg::Hybrid)]
        basis: EstimateBasisArg,
    },

    /// Append a completed run record (JSON) to local telemetry history.
    RecordRun {
        path: PathBuf,
    },

    /// Validate required documentation paths and source-of-truth references.
    DocsCheck,

    /// Execute a campaign. This is a scaffolded command in v0.
    Run {
        #[arg(long)]
        parent: u64,
        #[arg(long)]
        hours: u32,
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
enum EstimateBasisArg {
    Template,
    Local,
    Hybrid,
    Ai,
}

impl From<EstimateBasisArg> for EstimateBasis {
    fn from(value: EstimateBasisArg) -> Self {
        match value {
            EstimateBasisArg::Template => EstimateBasis::Template,
            EstimateBasisArg::Local => EstimateBasis::Local,
            EstimateBasisArg::Hybrid => EstimateBasis::Hybrid,
            EstimateBasisArg::Ai => EstimateBasis::Ai,
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::load(&cli.config)?;

    match cli.command {
        Commands::Budget { hours } => {
            let slots = budget::slots_for_hours(
                hours,
                config.loop_cfg.fallback_cycle_minutes,
                config.loop_cfg.fixed_overhead_minutes,
                config.loop_cfg.min_hours,
                config.loop_cfg.max_hours,
            )?;
            println!("hours={hours} fallback_slots={slots}");
        }
        Commands::LintIssue { path } => {
            let report = issue_lint::lint_markdown_issue(&path)?;
            println!("valid={}", report.valid);
            for missing in report.missing_sections {
                println!("missing={missing}");
            }
        }
        Commands::EstimateIssue { path, basis } => {
            let report = estimate::estimate_issue(&config, &path, basis.into())?;
            println!("model_profile={}", report.model_profile);
            println!("model={}", report.model);
            println!("reasoning_effort={}", report.reasoning_effort);
            println!("estimated_minutes={}", report.estimated_minutes);
            println!("recommended_single_issue_window_hours={}", report.recommended_hours);
            println!("basis_requested={}", report.basis_requested);
            println!("basis_used={}", report.basis_used);
            println!("local_samples={}", report.local_samples);
            if let Some(notes) = report.notes {
                println!("notes={notes}");
            }
        }
        Commands::RecordRun { path } => {
            let record = telemetry::read_run_record(&path)?;
            telemetry::append_run_record(&config.telemetry.history_path, &record)?;
            println!("ok=true");
            println!("history_path={}", config.telemetry.history_path.display());
        }
        Commands::DocsCheck => {
            let report = docs_support::check_docs(&config)?;
            println!("ok={}", report.ok);
            for missing in report.missing_paths {
                println!("missing={}", missing.display());
            }
        }
        Commands::Run {
            parent,
            hours,
            dry_run,
        } => {
            let fallback_slots = budget::slots_for_hours(
                hours,
                config.loop_cfg.fallback_cycle_minutes,
                config.loop_cfg.fixed_overhead_minutes,
                config.loop_cfg.min_hours,
                config.loop_cfg.max_hours,
            )?;
            println!("parent_issue={parent}");
            println!("hours={hours}");
            println!("fallback_slots={fallback_slots}");
            println!("dry_run={dry_run}");
            println!("next_step=implement_github_selection_issue_estimation_and_runner");
        }
    }

    Ok(())
}
