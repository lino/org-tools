// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! File collection utilities for recursively scanning directories for `.org` files.

use std::io::Write;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

/// Collects all `.org` files from the given paths, recursing into directories.
///
/// Respects `.gitignore` rules via the [`ignore`] crate. Files are returned
/// sorted by path for deterministic ordering.
pub fn collect_org_files(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut files = Vec::new();

    for path in paths {
        if path.is_file() {
            files.push(path.clone());
        } else if path.is_dir() {
            for entry in WalkBuilder::new(path).build().flatten() {
                let p = entry.path();
                if p.is_file() && p.extension().is_some_and(|ext| ext == "org") {
                    files.push(p.to_path_buf());
                }
            }
        } else {
            eprintln!("org: path not found: {}", path.display());
        }
    }

    files.sort();
    files
}

/// Atomically writes content to a file.
///
/// Creates a temporary file in the parent directory of `path`, writes `content`,
/// flushes and syncs to disk, then atomically renames the temporary file over
/// `path`. This prevents partial writes, file truncation to 0 bytes, or corruption
/// if interrupted mid-write. Existing file permissions are preserved where possible.
pub fn write_file_atomic(path: &Path, content: &str) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    if !parent.exists() {
        std::fs::create_dir_all(parent)?;
    }

    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(content.as_bytes())?;
    temp.as_file().sync_all()?;

    if let Ok(metadata) = std::fs::metadata(path) {
        let _ = temp.as_file().set_permissions(metadata.permissions());
    }

    temp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_file_atomic_new_and_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sub/test.org");

        // Write to new file (creates subdirectories automatically)
        write_file_atomic(&file, "initial content\n").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "initial content\n");

        // Overwrite atomically
        write_file_atomic(&file, "updated content\n").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "updated content\n");
    }
}
