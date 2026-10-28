//! src/main_components/vfs_cow.rs - Kernel Reflink & Copy-on-Write Engine.
//! Leverages APFS clonefile(2) on macOS and ioctl(FICLONE) on Linux with zero-copy fallback.

use std::fs;
use std::path::Path;
use std::time::Instant;
use walkdir::WalkDir;

use super::error::{NexBoxError, Result};

pub struct CowCloner;

impl CowCloner {
    /// Recursively duplicates a directory tree into destination using kernel-level reflinks.
    /// Excludes version control databases and prior sandbox artifacts.
    pub fn clone_tree(src: &Path, dst: &Path) -> Result<(usize, u128)> {
        let start = Instant::now();
        let mut cloned_files = 0usize;

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

        for entry in walker {
            let entry = entry.map_err(|e| {
                NexBoxError::Io("Walkdir traversal fault".into(), e.into())
            })?;
            let path = entry.path();
            let relative = path.strip_prefix(src).map_err(|e| {
                NexBoxError::ExecutionFailure(format!("Failed to compute relative path: {}", e))
            })?;

            if relative.as_os_str().is_empty() {
                continue;
            }

            let target_path = dst.join(relative);

            if entry.file_type().is_dir() {
                fs::create_dir_all(&target_path).map_err(|e| {
                    NexBoxError::Io(format!("Failed to create directory '{}'", target_path.display()), e)
                })?;
            } else if entry.file_type().is_symlink() {
                Self::clone_symlink(path, &target_path)?;
                cloned_files += 1;
            } else if entry.file_type().is_file() {
                Self::reflink_file(path, &target_path)?;
                cloned_files += 1;
            }
        }

        let elapsed_micros = start.elapsed().as_micros();
        Ok((cloned_files, elapsed_micros))
    }

    /// Single-file kernel reflink primitive.
    pub fn reflink_file(src: &Path, dst: &Path) -> Result<()> {
        if let Some(parent) = dst.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent).map_err(|e| {
                    NexBoxError::Io(format!("Failed to create parent dir '{}'", parent.display()), e)
                })?;
            }
        }

        // 1. Try Platform Native CoW Reflink
        #[cfg(target_os = "macos")]
        {
            if Self::macos_clonefile(src, dst).is_ok() {
                return Ok(());
            }
        }

        #[cfg(target_os = "linux")]
        {
            if Self::linux_ficlone(src, dst).is_ok() {
                return Ok(());
            }
        }

        // 2. High-speed Fallback: Hardlink if on same filesystem
        if dst.exists() {
            let _ = fs::remove_file(dst);
        }
        if fs::hard_link(src, dst).is_ok() {
            return Ok(());
        }

        // 3. Fallback: Standard zero-allocation byte copy
        fs::copy(src, dst).map_err(|e| {
            NexBoxError::Io(format!("Fallback copy failed for '{}'", src.display()), e)
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
}