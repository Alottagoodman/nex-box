//! src/main_components/cli.rs - Ergonomic CLI definition matching Astral/uv standards.

use clap::{Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StrategyOpt {
    Auto,
    Reflink,
    Overlay,
    Copy,
}

#[derive(Parser, Debug)]
#[command(
    name = "nex-box",
    author = "The Trinity",
    version,
    about = "Sub-millisecond Copy-on-Write ephemeral sandbox runtime for autonomous coding agents.",
    long_about = "nex-box provides zero-latency, kernel-level CoW execution sandboxes for AI agents.\n\
                  It allows agents to run builds, test suites, and destructive edits in isolated scratch\n\
                  environments backed by APFS/FICLONE reflinks or OverlayFS, guaranteeing instant atomic\n\
                  commits and clean rollbacks."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Execute a command inside an ephemeral zero-copy CoW sandbox.
    #[command(trailing_var_arg = true)]
    Run {
        /// Target command and arguments to execute inside the sandbox.
        #[arg(required = true, allow_hyphen_values = true)]
        command: Vec<String>,

        /// Base working directory to clone (defaults to current directory).
        #[arg(short = 'C', long = "cwd")]
        cwd: Option<String>,

        /// Automatically commit all scratch modifications back to host if command exits with 0.
        #[arg(short = 'c', long = "commit")]
        commit: bool,

        /// Wall-clock execution timeout in milliseconds (0 = unbounded).
        #[arg(short = 't', long = "timeout-ms", default_value_t = 60000)]
        timeout_ms: u64,

        /// Output full execution telemetry and file diff in structured JSON.
        #[arg(long = "json")]
        json: bool,

        /// Keep the ephemeral scratch directory after execution (default: purge immediately).
        #[arg(long = "preserve")]
        preserve: bool,

        /// Allow outbound network sockets (default: enabled; pass false to isolate network).
        #[arg(long = "allow-net", default_value_t = true)]
        allow_net: bool,

        /// Explicitly enforce isolation backend strategy (auto, reflink, overlay, copy).
        #[arg(long = "strategy", value_enum, default_value_t = StrategyOpt::Auto)]
        strategy: StrategyOpt,
    },

    /// Inspect the uncommitted file differences of an existing preserved sandbox session.
    Diff {
        /// Base working directory.
        #[arg(short = 'C', long = "cwd")]
        cwd: Option<String>,

        /// Active session identifier.
        #[arg(short = 's', long = "session", required = true)]
        session_id: String,
    },

    /// Purge all residual scratch directories from .nexbox/ in the current workspace.
    Clean {
        /// Base working directory.
        #[arg(short = 'C', long = "cwd")]
        cwd: Option<String>,
    },
}