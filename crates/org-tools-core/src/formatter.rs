// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::diagnostic::Fix;

/// Applies a sorted, non-overlapping list of fixes to produce formatted output.
///
/// Iterates left-to-right through the source, copying unmodified text and
/// substituting fix replacements at each span. Fixes must be sorted by
/// `span.start` ascending and must not overlap.
pub fn apply_fixes(content: &str, fixes: &[Fix]) -> String {
    if fixes.is_empty() {
        return content.to_string();
    }

    let mut result = String::with_capacity(content.len());
    let mut cursor = 0;

    for fix in fixes {
        debug_assert!(
            fix.span.start >= cursor,
            "Overlapping or out-of-order fixes: cursor={}, fix.start={}",
            cursor,
            fix.span.start
        );
        result.push_str(&content[cursor..fix.span.start]);
        result.push_str(&fix.replacement);
        cursor = fix.span.end;
    }

    result.push_str(&content[cursor..]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::Span;

    #[test]
    fn no_fixes() {
        assert_eq!(apply_fixes("hello world", &[]), "hello world");
    }

    #[test]
    fn single_deletion() {
        let fixes = vec![Fix::new(Span::new(5, 8), String::new())];
        assert_eq!(apply_fixes("hello   world", &fixes), "helloworld");
    }

    #[test]
    fn single_replacement() {
        let fixes = vec![Fix::new(Span::new(0, 5), "hi".to_string())];
        assert_eq!(apply_fixes("hello world", &fixes), "hi world");
    }

    #[test]
    fn multiple_fixes() {
        let fixes = vec![
            Fix::new(Span::new(3, 5), String::new()),
            Fix::new(Span::new(8, 10), String::new()),
        ];
        assert_eq!(apply_fixes("abc  def  ghi", &fixes), "abcdefghi");
    }

    #[test]
    fn fix_at_end() {
        let fixes = vec![Fix::new(Span::new(4, 7), String::new())];
        assert_eq!(apply_fixes("text   ", &fixes), "text");
    }
}
