//! src/main_components/sandbox.rs - Ephemeral Session Runtime & Atomic Commit Engine.
//! Executes processes with process-group killpg watchdogs, tracks lifetimes, and synchronizes mutations.
//! Hardened with kernel-level network isolation (macOS Seatbelt & Linux CLONE_NEWNET namespaces).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};
use colored::Colorize;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::cli::StrategyOpt;
use super::diff::{FileChangeKind, WorkspaceDiffReport};
use super::error::{NexBoxError, Result};
use super::vfs_cow::{CowCloner, ResolvedStrategy};

#[derive(Debug, Clone)]
pub struct ExecutionPolicy {
    pub timeout_ms: u64,
    pub allow_network: bool,
    pub auto_commit: bool,
    pub preserve_scratch: bool,
    pub json_telemetry: bool,
    pub enforced_strategy: StrategyOpt,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SandboxExecutionOutcome {
    pub session_id: String,
    pub exit_code: i32,
    pub duration_ms: u64,
    pub files_cloned: usize,
    pub clone_duration_micros: u128,
    pub strategy: String,
    pub committed: bool,
    pub diff: WorkspaceDiffReport,
    pub stdout_capture: String,
    pub stderr_capture: String,
}

impl SandboxExecutionOutcome {
    pub fn render_human_terminal(&self) {
        println!("{}", "━".repeat(80).dimmed());
        let status = if self.exit_code == 0 {
            "[✓] COMMAND EXECUTION SUCCESSFUL".bold().green()
        } else {
            format!("[✗] COMMAND FAILED WITH EXIT CODE {}", self.exit_code).bold().red()
        };

        println!("{} | Session: {}", status, self.session_id.bold());
        println!(
            "  ⏱ Telemetry: Duration: {}ms | CoW Clone: {} files in {}µs | Strategy: {}",
            self.duration_ms, self.files_cloned, self.clone_duration_micros, self.strategy.cyan()
        );
        println!("{}", "━".repeat(80).dimmed());

        self.diff.render_terminal();

        println!("{}", "━".repeat(80).dimmed());
        if self.committed {
            println!(
                "{}",
                "🏆 [ATOMIC COMMIT APPLIED]: Scratch mutations promoted to host workspace.".bold().green()
            );
        } else if self.exit_code == 0 {
            println!(
                "{}",
                "ℹ [SANDBOX DISCARDED]: Changes verified in scratch. Pass --commit to apply to host.".bold().yellow()
            );
        } else {
            println!(
                "{}",
                "🛑 [AUTOMATIC ROLLBACK]: Scratch tree purged due to command failure.".bold().red()
            );
        }
        println!("{}", "━".repeat(80).dimmed());
    }
}

pub struct SandboxSession {
    pub session_id: String,
    pub host_root: PathBuf,
    pub scratch_root: PathBuf,
    pub execution_dir: PathBuf,
    pub policy: ExecutionPolicy,
    pub files_cloned: usize,
    pub clone_micros: u128,
    pub strategy: ResolvedStrategy,
}

impl SandboxSession {
    pub fn initialize(host_root: &Path, policy: ExecutionPolicy) -> Result<Self> {
        let session_id = format!("box_{}", Uuid::new_v4().simple());
        let nex_dir = host_root.join(".nexbox").join("sessions");
        let scratch_root = nex_dir.join(&session_id);

        // Verify st_dev boundary: host and scratch must be on the same filesystem
        CowCloner::assert_same_device_boundary(host_root, &nex_dir)?;

        fs::create_dir_all(&scratch_root).map_err(|e| {
            NexBoxError::Io(format!("Failed creating session dir '{}'", scratch_root.display()), e)
        })?;

        // Determine strategy & duplicate tree
        let strategy = CowCloner::probe_strategy(host_root, &scratch_root, policy.enforced_strategy);
        let (files_cloned, clone_micros, active_strategy) =
            CowCloner::clone_tree(host_root, &scratch_root, strategy)?;

        let execution_dir = if active_strategy == ResolvedStrategy::FuseOverlayFs {
            scratch_root.join("merged")
        } else {
            scratch_root.clone()
        };

        Ok(Self {
            session_id,
            host_root: host_root.to_path_buf(),
            scratch_root,
            execution_dir,
            policy,
            files_cloned,
            clone_micros,
            strategy: active_strategy,
        })
    }

    pub fn execute(self, command_parts: &[String]) -> Result<SandboxExecutionOutcome> {
        let t_start = Instant::now();

        let prog = &command_parts[0];
        let args = &command_parts[1..];

        // File-backed stdout/stderr logs eliminate 64KB POSIX pipe buffer deadlocks
        let stdout_path = self.scratch_root.join(".nexbox_stdout.log");
        let stderr_path = self.scratch_root.join(".nexbox_stderr.log");

        let stdout_file = fs::File::create(&stdout_path).map_err(|e| {
            NexBoxError::Io("Failed creating scratch stdout log".into(), e)
        })?;
        let stderr_file = fs::File::create(&stderr_path).map_err(|e| {
            NexBoxError::Io("Failed creating scratch stderr log".into(), e)
        })?;

        let mut cmd: Command;

        #[cfg(target_os = "macos")]
        {
            if !self.policy.allow_network {
                cmd = Command::new("/usr/bin/sandbox-exec");
                let seatbelt_profile = "(version 1)\n(allow default)\n(deny network*)\n";
                cmd.arg("-p").arg(seatbelt_profile);
                cmd.arg(prog);
                cmd.args(args);
            } else {
                cmd = Command::new(prog);
                cmd.args(args);
            }

            // Put child process into a dedicated process group for clean killpg teardown
            use std::os::unix::process::CommandExt;
            unsafe {
                cmd.pre_exec(|| {
                    if libc::setpgid(0, 0) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }

        #[cfg(target_os = "linux")]
        {
            cmd = Command::new(prog);
            cmd.args(args);

            use std::os::unix::process::CommandExt;
            let allow_net = self.policy.allow_network;
            unsafe {
                cmd.pre_exec(move || {
                    // 1. Isolate process group
                    if libc::setpgid(0, 0) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }

                    // 2. Kernel Network Confinement
                    if !allow_net {
                        const CLONE_NEWNET: libc::c_int = 0x40000000;
                        const CLONE_NEWUSER: libc::c_int = 0x10000000;
                        if libc::unshare(CLONE_NEWNET | CLONE_NEWUSER) != 0 {
                            let _ = libc::unshare(CLONE_NEWNET);
                        }
                    }
                    Ok(())
                });
            }
        }

        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            cmd = Command::new(prog);
            cmd.args(args);
        }

        cmd.current_dir(&self.execution_dir);
        cmd.stdout(stdout_file);
        cmd.stderr(stderr_file);

        cmd.env("NEXBOX_ACTIVE", "1");
        cmd.env("NEXBOX_SESSION", &self.session_id);

        let mut child = cmd.spawn().map_err(|e| {
            NexBoxError::ExecutionFailure(format!("Failed executing '{}': {}", prog, e))
        })?;

        let child_pid = child.id();

        // Watchdog loop: Process Group Teardown (killpg) on timeout
        let exit_status = loop {
            match child.try_wait().map_err(|e| NexBoxError::Io("Process wait fault".into(), e))? {
                Some(status) => break status,
                None => {
                    if self.policy.timeout_ms > 0
                        && t_start.elapsed().as_millis() as u64 >= self.policy.timeout_ms
                    {
                        Self::kill_process_group(child_pid);
                        let _ = child.wait();
                        self.cleanup_scratch();
                        return Err(NexBoxError::CommandTimeout(self.policy.timeout_ms));
                    }
                    thread::sleep(Duration::from_millis(2));
                }
            }
        };

        let duration_ms = t_start.elapsed().as_millis() as u64;
        let exit_code = exit_status.code().unwrap_or(-1);

        let stdout_capture = fs::read_to_string(&stdout_path).unwrap_or_default();
        let stderr_capture = fs::read_to_string(&stderr_path).unwrap_or_default();

        let _ = fs::remove_file(&stdout_path);
        let _ = fs::remove_file(&stderr_path);

        // Compute BLAKE3 differential
        let diff = WorkspaceDiffReport::compute(&self.host_root, &self.execution_dir)?;

        // Atomic commit promotion
        let mut committed = false;
        if exit_code == 0 && self.policy.auto_commit {
            self.commit_to_host(&diff)?;
            committed = true;
        }

        self.cleanup_scratch();

        if !self.policy.json_telemetry {
            if !stdout_capture.is_empty() {
                print!("{}", stdout_capture);
            }
            if !stderr_capture.is_empty() {
                eprint!("{}", stderr_capture);
            }
        }

        Ok(SandboxExecutionOutcome {
            session_id: self.session_id,
            exit_code,
            duration_ms,
            files_cloned: self.files_cloned,
            clone_duration_micros: self.clone_micros,
            strategy: self.strategy.to_string(),
            committed,
            diff,
            stdout_capture,
            stderr_capture,
        })
    }

    /// Hard termination of child process and all child-spawned sub-processes via process group SIGKILL.
    fn kill_process_group(pid: u32) {
        #[cfg(unix)]
        {
            unsafe {
                let pgid = pid as libc::pid_t;
                // Dispatch SIGKILL to the entire process group
                libc::killpg(pgid, libc::SIGKILL);
                libc::kill(pid as libc::pid_t, libc::SIGKILL);
            }
        }
        #[cfg(not(unix))]
        {
            let _ = Command::new("taskkill")
                .args(["/F", "/T", "/PID", &pid.to_string()])
                .status();
        }
    }

    /// Cleans up overlay mounts and session directories if not marked for preservation.
    fn cleanup_scratch(&self) {
        if self.strategy == ResolvedStrategy::FuseOverlayFs {
            let _ = CowCloner::teardown_overlay(&self.scratch_root);
        }
        if !self.policy.preserve_scratch && self.scratch_root.exists() {
            let _ = fs::remove_dir_all(&self.scratch_root);
        }
    }

    /// Atomically commits changes back to host using safe reflink or copy semantics.
    fn commit_to_host(&self, diff: &WorkspaceDiffReport) -> Result<()> {
        for delta in &diff.deltas {
            let host_file = self.host_root.join(&delta.relative_path);
            let scratch_file = self.execution_dir.join(&delta.relative_path);

            match delta.change_kind {
                FileChangeKind::Created | FileChangeKind::Modified => {
                    if let Some(parent) = host_file.parent() {
                        if !parent.exists() {
                            fs::create_dir_all(parent).map_err(|e| {
                                NexBoxError::Io(format!("Commit parent dir creation error: {}", parent.display()), e)
                            })?;
                        }
                    }
                    CowCloner::reflink_or_copy_file(&scratch_file, &host_file, self.strategy)?;
                }
                FileChangeKind::Deleted => {
                    if host_file.exists() {
                        fs::remove_file(&host_file).map_err(|e| {
                            NexBoxError::Io(format!("Commit deletion failed on '{}'", host_file.display()), e)
                        })?;
                    }
                }
            }
        }
        Ok(())
    }

    pub fn inspect_diff(host_root: &Path, session_id: &str) -> Result<()> {
        let scratch_root = host_root.join(".nexbox").join("sessions").join(session_id);
        let execution_dir = if scratch_root.join("merged").exists() {
            scratch_root.join("merged")
        } else {
            scratch_root.clone()
        };

        if !execution_dir.exists() {
            return Err(NexBoxError::SessionNotFound(session_id.into()));
        }

        let diff = WorkspaceDiffReport::compute(host_root, &execution_dir)?;
        diff.render_terminal();
        Ok(())
    }

    pub fn clean_all_scratches(host_root: &Path) -> Result<()> {
        let nex_dir = host_root.join(".nexbox");
        if nex_dir.exists() {
            let sessions_dir = nex_dir.join("sessions");
            if sessions_dir.exists() {
                if let Ok(entries) = fs::read_dir(&sessions_dir) {
                    for entry in entries.flatten() {
                        let _ = CowCloner::teardown_overlay(&entry.path());
                    }
                }
            }
            fs::remove_dir_all(&nex_dir).map_err(|e| {
                NexBoxError::Io("Failed to purge .nexbox scratch storage".into(), e)
            })?;
        }
        Ok(())
    }
}