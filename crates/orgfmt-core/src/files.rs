// Copyright (C) 2026 orgfmt contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! File collection utilities for recursively scanning directories for `.org` files.

use std::path::PathBuf;

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
