//! src/main.rs - Pure Orchestrator Shell (<= 150 LOC)
//! Coordinates CLI parsing, session setup, sandbox execution, diff reporting, and atomic commits.

mod main_components;

use std::path::PathBuf;
use std::process::ExitCode;
use clap::Parser;
use colored::Colorize;

use main_components::cli::{Cli, Commands};
use main_components::error::{NexBoxError, Result};
use main_components::sandbox::{ExecutionPolicy, SandboxSession};

fn main() -> ExitCode {
    let cli = Cli::parse();

    match run_orchestrator(cli) {
        Ok(code) => ExitCode::from(code as u8),
        Err(err) => {
            eprintln!("{} {}", "[NEX-BOX FATAL]".bold().red(), err);
            ExitCode::from(1)
        }
    }
}

fn run_orchestrator(cli: Cli) -> Result<i32> {
    match cli.command {
        Commands::Run {
            command,
            cwd,
            commit,
            timeout_ms,
            json,
            preserve,
            allow_net,
        } => {
            if command.is_empty() {
                return Err(NexBoxError::ExecutionFailure("No command provided to execute.".into()));
            }

            let workspace_root = cwd
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
                .canonicalize()
                .map_err(|e| NexBoxError::Io("Failed to canonicalize workspace root".into(), e))?;

            let policy = ExecutionPolicy {
                timeout_ms,
                allow_network: allow_net,
                auto_commit: commit,
                preserve_scratch: preserve,
                json_telemetry: json,
            };

            let session = SandboxSession::initialize(&workspace_root, policy)?;
            let outcome = session.execute(&command)?;

            if json {
                println!("{}", serde_json::to_string_pretty(&outcome)?);
            } else {
                outcome.render_human_terminal();
            }

            Ok(outcome.exit_code)
        }

        Commands::Diff { cwd, session_id } => {
            let workspace_root = cwd
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
                .canonicalize()
                .map_err(|e| NexBoxError::Io("Failed to canonicalize workspace root".into(), e))?;

            SandboxSession::inspect_diff(&workspace_root, &session_id)?;
            Ok(0)
        }

        Commands::Clean { cwd } => {
            let workspace_root = cwd
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
                .canonicalize()
                .map_err(|e| NexBoxError::Io("Failed to canonicalize workspace root".into(), e))?;

            SandboxSession::clean_all_scratches(&workspace_root)?;
            println!("{} All ephemeral scratch spaces purged.", "[✓]".bold().green());
            Ok(0)
        }
    }
}