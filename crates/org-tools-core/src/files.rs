// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! File collection and atomic write utilities for `.org` workspaces.

use std::io::Write;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

/// Constructs a [`WalkBuilder`] for `root` with Windows `.git\info\exclude` verbatim-path support.
pub fn build_org_walk_builder(root: &Path) -> WalkBuilder {
    #[cfg(windows)]
    {
        let mut builder = WalkBuilder::new(root);
        let clean = strip_windows_verbatim(root.to_path_buf());
        for ancestor in clean.ancestors() {
            let exclude = ancestor.join(".git").join("info").join("exclude");
            if exclude.is_file() {
                let _ = builder.add_ignore(exclude);
                break;
            }
        }
        builder
    }
    #[cfg(not(windows))]
    {
        WalkBuilder::new(root)
    }
}

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
            for entry in build_org_walk_builder(path).build().flatten() {
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

/// Strips the Win32 `\\?\` extended-length path prefix when present so `/` and `starts_with` behave normally.
pub fn strip_windows_verbatim(path: PathBuf) -> PathBuf {
    let Some(s) = path.to_str() else {
        return path;
    };
    if let Some(unc_tail) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{unc_tail}"));
    }
    if let Some(tail) = s.strip_prefix(r"\\?\") {
        if tail.as_bytes().get(1) == Some(&b':') {
            return PathBuf::from(tail);
        }
    }
    path
}

/// Canonicalizes `path` via [`std::fs::canonicalize`] and strips any Win32 `\\?\` verbatim prefix.
pub fn canonicalize_clean(path: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(path).map(strip_windows_verbatim)
}

/// Joins a slash-separated relative path onto `base` component-by-component using native OS separators.
pub fn join_slash_rel(base: &Path, rel_path: &str) -> PathBuf {
    let mut out = strip_windows_verbatim(base.to_path_buf());
    for seg in rel_path.split(['/', '\\']) {
        if !seg.is_empty() && seg != "." {
            out.push(seg);
        }
    }
    out
}

/// Atomically writes `content` to `path` (or its canonical target when `path` is a symlink).
///
/// Creates a temporary file in the target's parent directory, writes `content`,
/// flushes and syncs to disk, preserves existing file permissions where possible,
/// and atomically renames the temporary file into place.
pub fn write_file_atomic(path: &Path, content: &str) -> std::io::Result<()> {
    let target = if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        canonicalize_clean(path)?
    } else {
        path.to_path_buf()
    };
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    if !parent.exists() {
        std::fs::create_dir_all(parent)?;
    }

    let normalized;
    let bytes = if content.contains('\r') {
        normalized = content.replace("\r\n", "\n").replace('\r', "\n");
        normalized.as_bytes()
    } else {
        content.as_bytes()
    };
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;

    if let Ok(metadata) = std::fs::metadata(&target) {
        let _ = temp.as_file().set_permissions(metadata.permissions());
    }

    temp.persist(&target).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies creating missing parent directories and overwriting a file atomically.
    #[test]
    fn test_write_file_atomic_new_and_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let file = join_slash_rel(dir.path(), "sub/test.org");

        write_file_atomic(&file, "initial content\n").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "initial content\n");

        write_file_atomic(&file, "updated content\n").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "updated content\n");
    }

    /// Verifies atomic writes through a symlink update the target file without unlinking the symlink.
    #[cfg(unix)]
    #[test]
    fn test_write_file_atomic_preserves_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.org");
        let alias = dir.path().join("alias.org");
        std::fs::write(&real, "* Old\n").unwrap();
        std::os::unix::fs::symlink(&real, &alias).unwrap();

        write_file_atomic(&alias, "* New\n").unwrap();
        assert!(std::fs::symlink_metadata(&alias)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "* New\n");
    }

    /// Verifies Win32 `\\?\` verbatim stripping and component-wise `/` path joining.
    #[test]
    fn test_strip_windows_verbatim_and_join_slash_rel() {
        assert_eq!(
            strip_windows_verbatim(PathBuf::from(r"\\?\C:\Users\runner\Temp\.tmp1")),
            PathBuf::from(r"C:\Users\runner\Temp\.tmp1")
        );
        assert_eq!(
            strip_windows_verbatim(PathBuf::from(r"\\?\UNC\server\share\notes")),
            PathBuf::from(r"\\server\share\notes")
        );
        let base = Path::new("/tmp/ws");
        assert_eq!(
            join_slash_rel(base, ".cache/org-cache.db"),
            base.join(".cache").join("org-cache.db")
        );
    }
}
