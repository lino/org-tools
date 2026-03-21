/// Shared list item parser for org-mode plain lists.
///
/// Spec: [§2.7 Plain Lists](https://orgmode.org/manual/Plain-Lists.html),
/// [Syntax: Plain Lists](https://orgmode.org/worg/org-syntax.html#Plain_Lists_and_Items)
///
/// Unordered: `-`, `+`, or `*` (only when indented) followed by space.
/// Ordered: `NUMBER.` or `NUMBER)` followed by space.
/// Checkboxes: `[ ]`, `[X]`, `[-]` immediately after the bullet.

#[derive(Debug, Clone, PartialEq)]
pub enum ListMarker {
    Dash,
    Plus,
    Star,
    OrderedDot(String),
    OrderedParen(String),
}

#[derive(Debug, PartialEq)]
pub struct ListItem<'a> {
    pub indent: usize,
    pub marker: ListMarker,
    pub checkbox: Option<char>,
    pub content: &'a str,
}

/// Parses a line as a list item. Returns None if the line is not a list item.
pub fn parse_list_item(line: &str) -> Option<ListItem<'_>> {
    let indent = line.len() - line.trim_start().len();
    let rest = &line[indent..];

    if rest.is_empty() {
        return None;
    }

    // Unordered markers.
    let (marker, after_marker) = if rest.starts_with("- ") || rest == "-" {
        (ListMarker::Dash, &rest[1..])
    } else if rest.starts_with("+ ") || rest == "+" {
        (ListMarker::Plus, &rest[1..])
    } else if indent > 0 && (rest.starts_with("* ") || rest == "*") {
        // `*` is only a list marker when indented (otherwise it's a heading).
        (ListMarker::Star, &rest[1..])
    } else if let Some((marker, after)) = parse_ordered_marker(rest) {
        (marker, after)
    } else {
        return None;
    };

    let after_marker = after_marker.strip_prefix(' ').unwrap_or(after_marker);

    // Extract checkbox.
    let (checkbox, content) = if after_marker.starts_with("[ ] ") || after_marker == "[ ]" {
        (Some(' '), after_marker[3..].trim_start())
    } else if after_marker.starts_with("[X] ") || after_marker == "[X]" {
        (Some('X'), after_marker[3..].trim_start())
    } else if after_marker.starts_with("[x] ") || after_marker == "[x]" {
        (Some('x'), after_marker[3..].trim_start())
    } else if after_marker.starts_with("[-] ") || after_marker == "[-]" {
        (Some('-'), after_marker[3..].trim_start())
    } else {
        (None, after_marker)
    };

    Some(ListItem {
        indent,
        marker,
        checkbox,
        content,
    })
}

/// Parses an ordered list marker like `1.`, `1)`, `a.`, `a)`.
fn parse_ordered_marker(text: &str) -> Option<(ListMarker, &str)> {
    let mut i = 0;
    let bytes = text.as_bytes();

    // Consume digits or a single letter.
    if i < bytes.len() && bytes[i].is_ascii_digit() {
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
    } else if i < bytes.len() && bytes[i].is_ascii_alphabetic() {
        i += 1; // Single letter.
    } else {
        return None;
    }

    if i >= bytes.len() {
        return None;
    }

    let number = &text[..i];
    let after = &text[i..];

    if after.starts_with(". ") || after == "." {
        Some((ListMarker::OrderedDot(number.to_string()), &after[1..]))
    } else if after.starts_with(") ") || after == ")" {
        Some((ListMarker::OrderedParen(number.to_string()), &after[1..]))
    } else {
        None
    }
}

/// Returns true if the line is a list item.
pub fn is_list_item(line: &str) -> bool {
    parse_list_item(line).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dash_item() {
        let item = parse_list_item("- Item text").unwrap();
        assert_eq!(item.indent, 0);
        assert_eq!(item.marker, ListMarker::Dash);
        assert_eq!(item.checkbox, None);
        assert_eq!(item.content, "Item text");
    }

    #[test]
    fn plus_item() {
        let item = parse_list_item("+ Item").unwrap();
        assert_eq!(item.marker, ListMarker::Plus);
    }

    #[test]
    fn star_item_indented() {
        let item = parse_list_item("  * Item").unwrap();
        assert_eq!(item.indent, 2);
        assert_eq!(item.marker, ListMarker::Star);
    }

    #[test]
    fn star_at_col0_is_not_list() {
        // Not indented → heading, not list item.
        assert!(parse_list_item("* Heading").is_none());
    }

    #[test]
    fn ordered_dot() {
        let item = parse_list_item("1. First item").unwrap();
        assert_eq!(item.marker, ListMarker::OrderedDot("1".to_string()));
        assert_eq!(item.content, "First item");
    }

    #[test]
    fn ordered_paren() {
        let item = parse_list_item("2) Second item").unwrap();
        assert_eq!(item.marker, ListMarker::OrderedParen("2".to_string()));
    }

    #[test]
    fn checkbox_empty() {
        let item = parse_list_item("- [ ] Not done").unwrap();
        assert_eq!(item.checkbox, Some(' '));
        assert_eq!(item.content, "Not done");
    }

    #[test]
    fn checkbox_done() {
        let item = parse_list_item("- [X] Done").unwrap();
        assert_eq!(item.checkbox, Some('X'));
    }

    #[test]
    fn checkbox_lowercase() {
        let item = parse_list_item("- [x] Done lowercase").unwrap();
        assert_eq!(item.checkbox, Some('x'));
    }

    #[test]
    fn checkbox_partial() {
        let item = parse_list_item("- [-] In progress").unwrap();
        assert_eq!(item.checkbox, Some('-'));
    }

    #[test]
    fn nested_item() {
        let item = parse_list_item("    - Nested").unwrap();
        assert_eq!(item.indent, 4);
    }

    #[test]
    fn not_a_list_item() {
        assert!(parse_list_item("just text").is_none());
        assert!(parse_list_item("").is_none());
        assert!(parse_list_item("  text").is_none());
    }

    #[test]
    fn letter_ordered() {
        let item = parse_list_item("a. First").unwrap();
        assert_eq!(item.marker, ListMarker::OrderedDot("a".to_string()));
    }
}
