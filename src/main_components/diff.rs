//! src/main_components/diff.rs - Fast In-Memory Differential Engine (BLAKE3 SIMD).
//! Compares Scratch workspace against Host root to compute exact Mutation Diffs.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use blake3::Hasher;
use colored::Colorize;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use super::error::{NexBoxError, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum FileChangeKind {
    Created,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDelta {
    pub relative_path: PathBuf,
    pub change_kind: FileChangeKind,
    pub original_sha256: Option<String>,
    pub new_sha256: Option<String>,
    pub byte_size_diff: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkspaceDiffReport {
    pub deltas: Vec<FileDelta>,
    pub files_created: usize,
    pub files_modified: usize,
    pub files_deleted: usize,
    pub elapsed_micros: u128,
}

impl WorkspaceDiffReport {
    pub fn compute(host_root: &Path, scratch_root: &Path) -> Result<Self> {
        let start = std::time::Instant::now();
        let mut deltas = Vec::new();

        let host_map = Self::scan_directory(host_root)?;
        let scratch_map = Self::scan_directory(scratch_root)?;

        let host_keys: HashSet<&PathBuf> = host_map.keys().collect();
        let scratch_keys: HashSet<&PathBuf> = scratch_map.keys().collect();

        // 1. Files Added in Scratch
        for added in scratch_keys.difference(&host_keys) {
            let scratch_meta = &scratch_map[*added];
            deltas.push(FileDelta {
                relative_path: (*added).clone(),
                change_kind: FileChangeKind::Created,
                original_sha256: None,
                new_sha256: Some(scratch_meta.hash.clone()),
                byte_size_diff: scratch_meta.size as i64,
            });
        }

        // 2. Files Deleted in Scratch
        for deleted in host_keys.difference(&scratch_keys) {
            let host_meta = &host_map[*deleted];
            deltas.push(FileDelta {
                relative_path: (*deleted).clone(),
                change_kind: FileChangeKind::Deleted,
                original_sha256: Some(host_meta.hash.clone()),
                new_sha256: None,
                byte_size_diff: -(host_meta.size as i64),
            });
        }

        // 3. Files in Both: Check for Modification
        for common in host_keys.intersection(&scratch_keys) {
            let h_meta = &host_map[*common];
            let s_meta = &scratch_map[*common];

            if h_meta.size != s_meta.size || h_meta.hash != s_meta.hash {
                deltas.push(FileDelta {
                    relative_path: (*common).clone(),
                    change_kind: FileChangeKind::Modified,
                    original_sha256: Some(h_meta.hash.clone()),
                    new_sha256: Some(s_meta.hash.clone()),
                    byte_size_diff: (s_meta.size as i64) - (h_meta.size as i64),
                });
            }
        }

        // Sort for deterministic presentation
        deltas.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));

        let files_created = deltas.iter().filter(|d| d.change_kind == FileChangeKind::Created).count();
        let files_modified = deltas.iter().filter(|d| d.change_kind == FileChangeKind::Modified).count();
        let files_deleted = deltas.iter().filter(|d| d.change_kind == FileChangeKind::Deleted).count();

        Ok(Self {
            deltas,
            files_created,
            files_modified,
            files_deleted,
            elapsed_micros: start.elapsed().as_micros(),
        })
    }

    fn scan_directory(root: &Path) -> Result<HashMap<PathBuf, FileRecord>> {
        let mut map = HashMap::new();
        if !root.exists() {
            return Ok(map);
        }

        let walker = WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                let name = e.file_name().to_string_lossy();
                !(name == ".git" || name == ".nexbox" || name == "target" || name == "node_modules" || name == ".DS_Store")
            });

        for entry in walker {
            let entry = entry.map_err(|e| NexBoxError::Io("Walkdir error during diff".into(), e.into()))?;
            if entry.file_type().is_file() {
                let full_path = entry.path();
                let rel = full_path.strip_prefix(root).map_err(|e| {
                    NexBoxError::ExecutionFailure(format!("Path prefix error: {}", e))
                })?.to_path_buf();

                let metadata = entry.metadata().map_err(|e| {
                    NexBoxError::Io(format!("Metadata read failed for '{}'", full_path.display()), e.into())
                })?;

                let size = metadata.len();
                let hash = Self::compute_file_hash(full_path)?;

                map.insert(rel, FileRecord { size, hash });
            }
        }

        Ok(map)
    }

    fn compute_file_hash(path: &Path) -> Result<String> {
        let mut hasher = Hasher::new();
        let bytes = fs::read(path).map_err(|e| {
            NexBoxError::Io(format!("Hash read failed for '{}'", path.display()), e)
        })?;
        hasher.update(&bytes);
        Ok(hasher.finalize().to_string())
    }

    pub fn render_terminal(&self) {
        if self.deltas.is_empty() {
            println!("  {}", "No file changes detected (Pristine working tree).".dimmed());
            return;
        }

        println!("  {}", "Discovered Mutations in Scratch Sandbox:".bold().cyan());
        for delta in &self.deltas {
            let path_str = delta.relative_path.display().to_string();
            match delta.change_kind {
                FileChangeKind::Created => {
                    println!("    {} {} (+{} bytes)", "+ [CREATED] ".bold().green(), path_str.bold(), delta.byte_size_diff);
                }
                FileChangeKind::Modified => {
                    let diff_str = if delta.byte_size_diff >= 0 {
                        format!("+{} bytes", delta.byte_size_diff)
                    } else {
                        format!("{} bytes", delta.byte_size_diff)
                    };
                    println!("    {} {} ({})", "~ [MODIFIED]".bold().yellow(), path_str.bold(), diff_str);
                }
                FileChangeKind::Deleted => {
                    println!("    {} {} ({} bytes)", "- [DELETED] ".bold().red(), path_str.bold(), delta.byte_size_diff);
                }
            }
        }
    }
}

struct FileRecord {
    size: u64,
    hash: String,
}