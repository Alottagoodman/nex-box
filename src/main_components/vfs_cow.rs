//! src/main_components/vfs_cow.rs - Kernel Reflink, OverlayFS & CoW Engine.
//! Handles cross-device verification, APFS clonefile(2), Linux ioctl(FICLONE),
//! and unprivileged fuse-overlayfs fallback for ext4 filesystems.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use super::cli::StrategyOpt;
use super::error::{NexBoxError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResolvedStrategy {
    ApfsReflink,
    LinuxFiclone,
    FuseOverlayFs,
    ParallelCopyFallback,
}

impl std::fmt::Display for ResolvedStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ApfsReflink => write!(f, "APFS clonefile(2) [Zero-Copy Extents]"),
            Self::LinuxFiclone => write!(f, "Linux ioctl(FICLONE) [Reflink Extents]"),
            Self::FuseOverlayFs => write!(f, "fuse-overlayfs [User-Namespace Cow]"),
            Self::ParallelCopyFallback => write!(f, "Rayon SIMD Parallel Copy [Non-CoW Ext4 Fallback]"),
        }
    }
}

pub struct CowCloner;

impl CowCloner {
    /// Validates that source and session target reside on the exact same filesystem st_dev.
    pub fn assert_same_device_boundary(src: &Path, session_parent: &Path) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let src_meta = fs::metadata(src).map_err(|e| {
                NexBoxError::Io(format!("Failed to stat workspace root '{}'", src.display()), e)
            })?;

            if !session_parent.exists() {
                fs::create_dir_all(session_parent).map_err(|e| {
                    NexBoxError::Io(format!("Failed creating session directory '{}'", session_parent.display()), e)
                })?;
            }

            let sess_meta = fs::metadata(session_parent).map_err(|e| {
                NexBoxError::Io(format!("Failed to stat session storage root '{}'", session_parent.display()), e)
            })?;

            if src_meta.dev() != sess_meta.dev() {
                return Err(NexBoxError::CrossDeviceBoundary {
                    workspace_dev: src_meta.dev(),
                    session_dev: sess_meta.dev(),
                });
            }
        }
        Ok(())
    }

    /// Determines the optimal cloning strategy based on host OS, filesystem features, and user options.
    pub fn probe_strategy(_src: &Path, _scratch_dir: &Path, requested: StrategyOpt) -> ResolvedStrategy {
        match requested {
            StrategyOpt::Reflink => {
                #[cfg(target_os = "macos")]
                return ResolvedStrategy::ApfsReflink;
                #[cfg(target_os = "linux")]
                return ResolvedStrategy::LinuxFiclone;
                #[cfg(not(any(target_os = "macos", target_os = "linux")))]
                return ResolvedStrategy::ParallelCopyFallback;
            }
            StrategyOpt::Overlay => ResolvedStrategy::FuseOverlayFs,
            StrategyOpt::Copy => ResolvedStrategy::ParallelCopyFallback,
            StrategyOpt::Auto => {
                #[cfg(target_os = "macos")]
                {
                    ResolvedStrategy::ApfsReflink
                }
                #[cfg(target_os = "linux")]
                {
                    if Self::probe_linux_ficlone_support(src, scratch_dir) {
                        ResolvedStrategy::LinuxFiclone
                    } else if which::which("fuse-overlayfs").is_ok() {
                        ResolvedStrategy::FuseOverlayFs
                    } else {
                        ResolvedStrategy::ParallelCopyFallback
                    }
                }
                #[cfg(not(any(target_os = "macos", target_os = "linux")))]
                {
                    ResolvedStrategy::ParallelCopyFallback
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn probe_linux_ficlone_support(src: &Path, scratch_dir: &Path) -> bool {
        let probe_src = scratch_dir.join(".ficlone_probe_src");
        let probe_dst = scratch_dir.join(".ficlone_probe_dst");

        let _ = fs::create_dir_all(scratch_dir);
        if fs::write(&probe_src, b"ficlone_check").is_err() {
            return false;
        }

        let supported = Self::linux_ficlone(&probe_src, &probe_dst).is_ok();
        let _ = fs::remove_file(&probe_src);
        let _ = fs::remove_file(&probe_dst);
        let _ = src;
        supported
    }

    /// Duplicates tree into scratch workspace honoring the resolved strategy.
    pub fn clone_tree(
        src: &Path,
        dst: &Path,
        strategy: ResolvedStrategy,
    ) -> Result<(usize, u128, ResolvedStrategy)> {
        let start = Instant::now();

        match strategy {
            ResolvedStrategy::FuseOverlayFs => {
                Self::setup_fuse_overlay(src, dst)?;
                let elapsed = start.elapsed().as_micros();
                Ok((0, elapsed, ResolvedStrategy::FuseOverlayFs))
            }
            other_strategy => {
                let count = Self::clone_tree_files(src, dst, other_strategy)?;
                let elapsed = start.elapsed().as_micros();
                Ok((count, elapsed, other_strategy))
            }
        }
    }

    fn clone_tree_files(src: &Path, dst: &Path, strategy: ResolvedStrategy) -> Result<usize> {
        if !dst.exists() {
            fs::create_dir_all(dst).map_err(|e| {
                NexBoxError::Io(format!("Failed to create destination root '{}'", dst.display()), e)
            })?;
        }

        let walker = WalkDir::new(src)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| {
                let name = entry.file_name().to_string_lossy();
                !(name == ".git" || name == ".nexbox" || name == "target" || name == "node_modules" || name == ".DS_Store")
            });

        let mut entries = Vec::new();
        for entry in walker {
            let entry = entry.map_err(|e| NexBoxError::Io("Walkdir traversal fault".into(), e.into()))?;
            entries.push(entry);
        }

        // Phase 1: Replicate directory structure sequentially
        for entry in &entries {
            if entry.file_type().is_dir() {
                let relative = entry.path().strip_prefix(src).map_err(|e| {
                    NexBoxError::ExecutionFailure(format!("Failed computing relative path: {}", e))
                })?;
                if !relative.as_os_str().is_empty() {
                    let target_dir = dst.join(relative);
                    fs::create_dir_all(&target_dir).map_err(|e| {
                        NexBoxError::Io(format!("Failed creating directory '{}'", target_dir.display()), e)
                    })?;
                }
            }
        }

        // Phase 2: Parallel clone/reflink files using Rayon
        let cloned_counter = AtomicUsize::new(0);
        let files_to_clone: Vec<_> = entries
            .into_iter()
            .filter(|e| !e.file_type().is_dir())
            .collect();

        files_to_clone
            .into_par_iter()
            .try_for_each(|entry| -> Result<()> {
                let path = entry.path();
                let relative = path.strip_prefix(src).map_err(|e| {
                    NexBoxError::ExecutionFailure(format!("Relative prefix error: {}", e))
                })?;
                let target_path = dst.join(relative);

                if entry.file_type().is_symlink() {
                    Self::clone_symlink(path, &target_path)?;
                    cloned_counter.fetch_add(1, Ordering::Relaxed);
                } else if entry.file_type().is_file() {
                    Self::reflink_or_copy_file(path, &target_path, strategy)?;
                    cloned_counter.fetch_add(1, Ordering::Relaxed);
                }
                Ok(())
            })?;

        Ok(cloned_counter.load(Ordering::SeqCst))
    }

    /// Single-file reflink primitive with strict anti-corruption semantics.
    pub fn reflink_or_copy_file(src: &Path, dst: &Path, strategy: ResolvedStrategy) -> Result<()> {
        if let Some(parent) = dst.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent).map_err(|e| {
                    NexBoxError::Io(format!("Failed to create parent dir '{}'", parent.display()), e)
                })?;
            }
        }

        match strategy {
            ResolvedStrategy::ApfsReflink => {
                #[cfg(target_os = "macos")]
                {
                    if Self::macos_clonefile(src, dst).is_ok() {
                        return Ok(());
                    }
                }
            }
            ResolvedStrategy::LinuxFiclone => {
                #[cfg(target_os = "linux")]
                {
                    if Self::linux_ficlone(src, dst).is_ok() {
                        return Ok(());
                    }
                }
            }
            _ => {}
        }

        // Fallback: Safe byte copy. NEVER use raw hardlinks to avoid in-place edit hazards!
        if dst.exists() {
            let _ = fs::remove_file(dst);
        }
        fs::copy(src, dst).map_err(|e| {
            NexBoxError::Io(format!("Zero-copy/fallback copy failed for '{}'", src.display()), e)
        })?;

        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn macos_clonefile(src: &Path, dst: &Path) -> std::result::Result<(), ()> {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        if dst.exists() {
            let _ = fs::remove_file(dst);
        }

        let src_c = CString::new(src.as_os_str().as_bytes()).map_err(|_| ())?;
        let dst_c = CString::new(dst.as_os_str().as_bytes()).map_err(|_| ())?;

        let ret = unsafe { libc::clonefile(src_c.as_ptr(), dst_c.as_ptr(), 0) };
        if ret == 0 {
            Ok(())
        } else {
            Err(())
        }
    }

    #[cfg(target_os = "linux")]
    fn linux_ficlone(src: &Path, dst: &Path) -> std::result::Result<(), ()> {
        use std::fs::OpenOptions;
        use std::os::unix::io::AsRawFd;

        if dst.exists() {
            let _ = fs::remove_file(dst);
        }

        let src_file = OpenOptions::new().read(true).open(src).map_err(|_| ())?;
        let dst_file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(dst)
            .map_err(|_| ())?;

        const FICLONE: libc::c_ulong = 0x40049409;
        let ret = unsafe {
            libc::ioctl(dst_file.as_raw_fd(), FICLONE as _, src_file.as_raw_fd())
        };

        if ret == 0 {
            if let Ok(meta) = src.metadata() {
                let _ = fs::set_permissions(dst, meta.permissions());
            }
            Ok(())
        } else {
            Err(())
        }
    }

    fn clone_symlink(src: &Path, dst: &Path) -> Result<()> {
        if dst.exists() {
            let _ = fs::remove_file(dst);
        }

        let link_target = fs::read_link(src).map_err(|e| {
            NexBoxError::Io(format!("Failed reading symlink '{}'", src.display()), e)
        })?;

        #[cfg(unix)]
        std::os::unix::fs::symlink(&link_target, dst).map_err(|e| {
            NexBoxError::Io(format!("Failed creating symlink '{}'", dst.display()), e)
        })?;

        Ok(())
    }

    /// Sets up unprivileged fuse-overlayfs mounting for non-reflink Linux filesystems.
    fn setup_fuse_overlay(lower_host: &Path, scratch_root: &Path) -> Result<()> {
        let upper = scratch_root.join("upper");
        let work = scratch_root.join("work");
        let merged = scratch_root.join("merged");

        fs::create_dir_all(&upper).map_err(|e| NexBoxError::Io("Create upper failed".into(), e))?;
        fs::create_dir_all(&work).map_err(|e| NexBoxError::Io("Create work failed".into(), e))?;
        fs::create_dir_all(&merged).map_err(|e| NexBoxError::Io("Create merged failed".into(), e))?;

        let options = format!(
            "lowerdir={},upperdir={},workdir={}",
            lower_host.display(),
            upper.display(),
            work.display()
        );

        let status = Command::new("fuse-overlayfs")
            .arg("-o")
            .arg(&options)
            .arg(&merged)
            .status()
            .map_err(|e| NexBoxError::OverlayFsMountFailed(format!("fuse-overlayfs spawn failure: {}", e)))?;

        if !status.success() {
            return Err(NexBoxError::OverlayFsMountFailed(format!(
                "fuse-overlayfs process exited with code {:?}",
                status.code()
            )));
        }

        Ok(())
    }

    /// Teardown handler for OverlayFS scratch mounts.
    pub fn teardown_overlay(scratch_root: &Path) -> Result<()> {
        let merged = scratch_root.join("merged");
        if merged.exists() {
            let _ = Command::new("fusermount3").arg("-u").arg(&merged).status();
            let _ = Command::new("fusermount").arg("-u").arg(&merged).status();
        }
        Ok(())
    }
}