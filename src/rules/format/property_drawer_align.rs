// Copyright (C) 2026 orgfmt contributors
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::diagnostic::{Fix, Span};
use crate::rules::{FormatContext, FormatRule};

/// Aligns property values within `:PROPERTIES:` drawers.
///
/// Spec: [Property Syntax](https://orgmode.org/worg/org-syntax.html#Property_Drawers),
/// [§7.1 Property Syntax](https://orgmode.org/manual/Property-Syntax.html)
///
/// When a property drawer contains multiple properties, this rule pads
/// the space after each `:KEY:` so that all values start at the same
/// column. Drawers with only a single property are left unchanged.
///
/// Example:
/// ```text
/// :PROPERTIES:
/// :ID:        abc
/// :CUSTOM_ID: my-section
/// :END:
/// ```
///
/// Rule ID: `F005`
pub struct PropertyDrawerAlign;

impl FormatRule for PropertyDrawerAlign {
    fn id(&self) -> &'static str {
        "F005"
    }

    fn name(&self) -> &'static str {
        "property-drawer-align"
    }

    fn description(&self) -> &'static str {
        "Align property values within drawers"
    }

    fn format(&self, ctx: &FormatContext) -> Vec<Fix> {
        let content = &ctx.source.content;
        let mut fixes = Vec::new();
        let lines: Vec<&str> = content.split('\n').collect();
        let mut i = 0;

        while i < lines.len() {
            let raw = lines[i].strip_suffix('\r').unwrap_or(lines[i]);
            if raw.trim() == ":PROPERTIES:" {
                let mut props: Vec<(usize, String, String)> = Vec::new(); // (line_idx, key, value)
                let mut j = i + 1;
                let mut found_end = false;

                while j < lines.len() {
                    let pline = lines[j].strip_suffix('\r').unwrap_or(lines[j]);
                    if pline.trim() == ":END:" {
                        found_end = true;
                        break;
                    }
                    if let Some((key, value)) = parse_property_line(pline) {
                        props.push((j, key, value));
                    }
                    j += 1;
                }

                if found_end && props.len() > 1 {
                    let max_key_len = props.iter().map(|(_, k, _)| k.len()).max().unwrap_or(0);

                    for &(line_idx, ref key, ref value) in &props {
                        // Pad after the closing colon so values align.
                        // Format: `:KEY:` then spaces then value
                        // `:KEY:` has length key.len() + 2
                        // We want all values to start at the same column.
                        let key_field_len = max_key_len + 2; // `:` + key + `:`
                        let current_key_field_len = key.len() + 2;
                        let padding = key_field_len - current_key_field_len;
                        let pad_str: String = " ".repeat(padding);
                        let formatted = format!(":{}:{}{}{}", key, pad_str, " ", value);
                        let original = lines[line_idx]
                            .strip_suffix('\r')
                            .unwrap_or(lines[line_idx]);

                        if formatted != original.trim_start() {
                            let line_start: usize =
                                lines[..line_idx].iter().map(|l| l.len() + 1).sum();
                            let leading_ws = original.len() - original.trim_start().len();
                            let prop_start = line_start + leading_ws;
                            let prop_end = line_start + original.len();

                            fixes.push(Fix::new(Span::new(prop_start, prop_end), formatted));
                        }
                    }
                }

                i = if found_end { j + 1 } else { j };
            } else {
                i += 1;
            }
        }

        fixes
    }
}

/// Parse a property line `:KEY: value` and return (key, value).
fn parse_property_line(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    if !trimmed.starts_with(':') {
        return None;
    }
    let rest = &trimmed[1..];
    let colon_pos = rest.find(':')?;
    let key = &rest[..colon_pos];
    if key.is_empty() {
        return None;
    }
    let value = rest[colon_pos + 1..].trim().to_string();
    Some((key.to_string(), value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::formatter::apply_fixes;
    use crate::source::SourceFile;

    fn format_it(input: &str) -> String {
        let source = SourceFile::new("test.org", input.to_string());
        let config = Config::default();
        let ctx = FormatContext::new(&source, &config);
        let fixes = PropertyDrawerAlign.format(&ctx);
        apply_fixes(input, &fixes)
    }

    #[test]
    fn aligns_properties() {
        let input = ":PROPERTIES:\n:ID: abc\n:CUSTOM_ID: my-section\n:END:\n";
        let expected = ":PROPERTIES:\n:ID:        abc\n:CUSTOM_ID: my-section\n:END:\n";
        assert_eq!(format_it(input), expected);
    }

    #[test]
    fn already_aligned() {
        let input = ":PROPERTIES:\n:ID:        abc\n:CUSTOM_ID: xyz\n:END:\n";
        assert_eq!(format_it(input), input);
    }

    #[test]
    fn single_property_no_change() {
        let input = ":PROPERTIES:\n:ID: abc\n:END:\n";
        assert_eq!(format_it(input), input);
    }

    #[test]
    fn no_drawers() {
        let input = "* Heading\ntext\n";
        assert_eq!(format_it(input), input);
    }

    #[test]
    fn multiple_drawers() {
        let input = concat!(
            ":PROPERTIES:\n:A: 1\n:BB: 2\n:END:\n",
            "text\n",
            ":PROPERTIES:\n:CCC: 3\n:D: 4\n:END:\n"
        );
        let expected = concat!(
            ":PROPERTIES:\n:A:  1\n:BB: 2\n:END:\n",
            "text\n",
            ":PROPERTIES:\n:CCC: 3\n:D:   4\n:END:\n"
        );
        assert_eq!(format_it(input), expected);
    }
}
