// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

/// Identifies protected regions in org source where formatting rules must not modify content.
///
/// Protected regions include `#+BEGIN_SRC`, `#+BEGIN_EXAMPLE`, `#+BEGIN_EXPORT`,
/// `#+BEGIN_QUOTE`, `#+BEGIN_VERSE`, `#+BEGIN_CENTER`, and `#+BEGIN_COMMENT` blocks.
///
/// Returns a sorted list of (start_line, end_line) pairs (0-based, inclusive).
pub fn protected_regions(content: &str) -> Vec<(usize, usize)> {
    let mut regions = Vec::new();
    let mut block_start: Option<usize> = None;

    for (i, line) in content.split('\n').enumerate() {
        let trimmed = line.trim_start();
        if block_start.is_some() {
            if trimmed.starts_with("#+END_") || trimmed.starts_with("#+end_") {
                if let Some(start) = block_start.take() {
                    regions.push((start, i));
                }
            }
        } else if trimmed.starts_with("#+BEGIN_SRC")
            || trimmed.starts_with("#+begin_src")
            || trimmed.starts_with("#+BEGIN_EXAMPLE")
            || trimmed.starts_with("#+begin_example")
            || trimmed.starts_with("#+BEGIN_EXPORT")
            || trimmed.starts_with("#+begin_export")
            || trimmed.starts_with("#+BEGIN_QUOTE")
            || trimmed.starts_with("#+begin_quote")
            || trimmed.starts_with("#+BEGIN_VERSE")
            || trimmed.starts_with("#+begin_verse")
            || trimmed.starts_with("#+BEGIN_CENTER")
            || trimmed.starts_with("#+begin_center")
            || trimmed.starts_with("#+BEGIN_COMMENT")
            || trimmed.starts_with("#+begin_comment")
        {
            block_start = Some(i);
        }
    }

    regions
}

/// Returns true if the given line (0-based) is inside a protected region.
pub fn is_protected(line: usize, regions: &[(usize, usize)]) -> bool {
    regions
        .iter()
        .any(|&(start, end)| line >= start && line <= end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_src_block() {
        let content = "text\n#+BEGIN_SRC python\ncode\n#+END_SRC\ntext";
        let regions = protected_regions(content);
        assert_eq!(regions, vec![(1, 3)]);
        assert!(!is_protected(0, &regions));
        assert!(is_protected(1, &regions));
        assert!(is_protected(2, &regions));
        assert!(is_protected(3, &regions));
        assert!(!is_protected(4, &regions));
    }

    #[test]
    fn detects_lowercase_blocks() {
        let content = "#+begin_example\nstuff\n#+end_example";
        let regions = protected_regions(content);
        assert_eq!(regions, vec![(0, 2)]);
    }

    #[test]
    fn multiple_blocks() {
        let content = "#+BEGIN_SRC\na\n#+END_SRC\ntext\n#+BEGIN_EXAMPLE\nb\n#+END_EXAMPLE";
        let regions = protected_regions(content);
        assert_eq!(regions, vec![(0, 2), (4, 6)]);
    }

    #[test]
    fn no_blocks() {
        let content = "just\nplain\ntext";
        let regions = protected_regions(content);
        assert!(regions.is_empty());
    }
}
