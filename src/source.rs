// Copyright (C) 2026 orgfmt contributors
// SPDX-License-Identifier: GPL-3.0-or-later

use std::path::{Path, PathBuf};

/// In-memory representation of an org-mode source file with precomputed line index.
///
/// Stores the file path, raw content, and a table of line-start byte offsets for
/// efficient `O(log n)` line/column lookups.
#[derive(Debug)]
pub struct SourceFile {
    /// Path to the source file on disk (or a synthetic path for tests).
    pub path: PathBuf,
    /// Raw file content as a UTF-8 string.
    pub content: String,
    /// Byte offsets of each line start, computed on construction.
    line_starts: Vec<usize>,
}

impl SourceFile {
    /// Creates a new source file from a path and content string.
    pub fn new(path: impl Into<PathBuf>, content: String) -> Self {
        let line_starts = Self::compute_line_starts(&content);
        Self {
            path: path.into(),
            content,
            line_starts,
        }
    }

    /// Reads a file from disk and constructs a `SourceFile`.
    pub fn from_path(path: &Path) -> std::io::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Ok(Self::new(path.to_path_buf(), content))
    }

    fn compute_line_starts(content: &str) -> Vec<usize> {
        let mut starts = vec![0];
        for (i, byte) in content.bytes().enumerate() {
            if byte == b'\n' {
                starts.push(i + 1);
            }
        }
        starts
    }

    /// Returns (1-based line, 1-based column) for a byte offset.
    pub fn line_col(&self, offset: usize) -> (usize, usize) {
        let line = match self.line_starts.binary_search(&offset) {
            Ok(idx) => idx,
            Err(idx) => idx - 1,
        };
        let col = offset - self.line_starts[line];
        (line + 1, col + 1)
    }

    /// Returns the byte offset where the given 0-based line starts.
    pub fn line_start(&self, line: usize) -> usize {
        self.line_starts[line]
    }

    /// Returns the number of lines.
    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// Returns an iterator over (line_index, line_text) pairs.
    /// Line text does NOT include the trailing newline.
    pub fn lines(&self) -> impl Iterator<Item = (usize, &str)> {
        self.content.split('\n').enumerate().map(|(i, line)| {
            // Handle \r\n line endings
            let line = line.strip_suffix('\r').unwrap_or(line);
            (i, line)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_col_basic() {
        let src = SourceFile::new("test.org", "abc\ndef\nghi\n".to_string());
        assert_eq!(src.line_col(0), (1, 1)); // 'a'
        assert_eq!(src.line_col(3), (1, 4)); // '\n'
        assert_eq!(src.line_col(4), (2, 1)); // 'd'
        assert_eq!(src.line_col(8), (3, 1)); // 'g'
    }

    #[test]
    fn line_count() {
        let src = SourceFile::new("test.org", "a\nb\nc\n".to_string());
        assert_eq!(src.line_count(), 4); // 3 lines + trailing empty
    }

    #[test]
    fn lines_iterator() {
        let src = SourceFile::new("test.org", "abc\ndef\nghi".to_string());
        let lines: Vec<_> = src.lines().collect();
        assert_eq!(lines, vec![(0, "abc"), (1, "def"), (2, "ghi")]);
    }
}
