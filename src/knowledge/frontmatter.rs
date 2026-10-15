//! Minimal YAML frontmatter parser/serializer for Obsidian-compatible Markdown.
//!
//! No external YAML dependency — the frontmatter subset used by Obsidian notes
//! is simple enough to parse by hand: key-value pairs, inline lists, and
//! block lists. This avoids adding `serde_yaml` to the dependency tree.

use std::collections::HashMap;

/// Parsed YAML frontmatter for an Obsidian-compatible Markdown note.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Frontmatter {
    pub title: String,
    pub tags: Vec<String>,
    pub created: Option<String>,
    pub modified: Option<String>,
    pub other: HashMap<String, String>,
}

/// The result of splitting a Markdown document into its frontmatter and body.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedNote {
    pub frontmatter: Frontmatter,
    pub content: String,
}

/// Sentinel that opens a YAML frontmatter block.
const FRONTMATTER_DELIM: &str = "---";

/// Parse a Markdown document that may contain YAML frontmatter.
///
/// If the content starts with `---` on the first line and has a matching
/// closing `---` (or `...`), the frontmatter is parsed and the body is
/// everything after the closing delimiter. Otherwise the entire content is
/// treated as the body with default (empty) frontmatter.
///
/// ```
/// use study_platform::knowledge::frontmatter::parse_note;
///
/// let md = "---\ntitle: Hello\ntags: [a, b]\n---\n\nWorld";
/// let parsed = parse_note(md);
/// assert_eq!(parsed.frontmatter.title, "Hello");
/// assert_eq!(parsed.frontmatter.tags, vec!["a".to_string(), "b".to_string()]);
/// assert_eq!(parsed.content.trim(), "World");
/// ```
pub fn parse_note(content: &str) -> ParsedNote {
    let trimmed_start = content.trim_start_matches('\n').trim_start();

    // Check for opening delimiter on the first non-empty line
    if !trimmed_start.starts_with(FRONTMATTER_DELIM) {
        // No frontmatter — entire content is the body
        return ParsedNote {
            frontmatter: Frontmatter::default(),
            content: content.to_string(),
        };
    }

    // Find the line that starts the opening delimiter
    // After the opening `---`, collect lines until the closing delimiter
    let lines: Vec<&str> = content.lines().collect();

    // Find closing delimiter (line that is exactly --- or ...)
    let mut close_idx: Option<usize> = None;
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if i == 0 && trimmed == FRONTMATTER_DELIM {
            // Opening delimiter on line 0 — look for closing after it
            for (j, line2) in lines.iter().enumerate().skip(1) {
                if line2.trim() == FRONTMATTER_DELIM || line2.trim() == "..." {
                    close_idx = Some(j);
                    break;
                }
            }
            break;
        }
        // Opening delimiter might be on a later line (preceded by blank lines)
        if trimmed == FRONTMATTER_DELIM {
            for (j, line2) in lines.iter().enumerate().skip(i + 1) {
                if line2.trim() == FRONTMATTER_DELIM || line2.trim() == "..." {
                    close_idx = Some(j);
                    break;
                }
            }
            let yaml_lines = &lines[i + 1..close_idx.unwrap_or(lines.len())];
            let body_lines = if close_idx.is_some() {
                &lines[close_idx.unwrap() + 1..]
            } else {
                &[]
            };
            let body = body_lines.join("\n").trim().to_string();
            return ParsedNote {
                frontmatter: parse_yaml_frontmatter(&yaml_lines.join("\n")),
                content: body,
            };
        }
    }

    if let Some(close) = close_idx {
        // Opening was line 0
        let yaml_lines: Vec<&str> = lines.iter().skip(1).take(close - 1).copied().collect();
        let body_lines: Vec<&str> = lines.iter().skip(close + 1).copied().collect();
        ParsedNote {
            frontmatter: parse_yaml_frontmatter(&yaml_lines.join("\n")),
            content: body_lines.join("\n").trim().to_string(),
        }
    } else {
        // No closing delimiter — treat entire content as body
        ParsedNote {
            frontmatter: Frontmatter::default(),
            content: content.to_string(),
        }
    }
}

/// Parse a YAML frontmatter block (without the `---` delimiters) into a
/// [`Frontmatter`] struct.
///
/// Supports:
/// - `key: value` pairs (quoted or unquoted)
/// - inline lists: `key: [item1, item2]`
/// - block lists:
///   ```yaml
///   key:
///     - item1
///     - item2
///   ```
/// - `#` comment lines (skipped)
fn parse_yaml_frontmatter(yaml: &str) -> Frontmatter {
    let mut fm = Frontmatter::default();
    let mut current_block_key: Option<String> = None;

    for line in yaml.lines() {
        // Skip blank lines and comment-only lines
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Block list item: "- value" (indented under a key)
        if trimmed.starts_with("- ") || trimmed == "-" {
            if let Some(ref key) = current_block_key {
                let item = unquote(trimmed.strip_prefix("- ").unwrap_or("").trim());
                match key.as_str() {
                    "tags" => fm.tags.push(item.clone()),
                    _ => {
                        let existing = fm.other.get(key).cloned().unwrap_or_default();
                        let updated = if existing.is_empty() {
                            item
                        } else {
                            format!("{}, {}", existing, item)
                        };
                        fm.other.insert(key.clone(), updated);
                    }
                }
            }
            continue;
        }

        current_block_key = None;

        // key: value pair
        if let Some(colon_pos) = trimmed.find(':') {
            let key = trimmed[..colon_pos].trim().to_string();
            let value = trimmed[colon_pos + 1..].trim();

            if value.is_empty() {
                // Could be start of a block list (e.g., "tags:")
                current_block_key = Some(key.clone());
                // Initialize tags list if it's the tags key
                if key == "tags" && fm.tags.is_empty() {
                    // will be populated by subsequent "- " lines
                }
            } else if value.starts_with('[') && value.ends_with(']') {
                // Inline list: [item1, item2]
                let inner = &value[1..value.len() - 1];
                let items: Vec<String> = inner
                    .split(',')
                    .map(|s| unquote(s.trim()))
                    .filter(|s| !s.is_empty())
                    .collect();
                match key.as_str() {
                    "tags" => fm.tags = items,
                    _ => {
                        fm.other.insert(key, items.join(", "));
                    }
                }
            } else {
                // Scalar value
                let val = unquote(value);
                match key.as_str() {
                    "title" => fm.title = val,
                    "tags" => {
                        // Single tag on same line
                        if !val.is_empty() {
                            fm.tags = vec![val];
                        }
                    }
                    "created" => fm.created = Some(val),
                    "modified" => fm.modified = Some(val),
                    _ => {
                        fm.other.insert(key, val);
                    }
                }
            }
        }
    }

    fm
}

/// Remove surrounding quotes from a YAML scalar value.
fn unquote(s: &str) -> String {
    if s.len() >= 2 {
        let first = s.chars().next().unwrap();
        let last = s.chars().last().unwrap();
        if (first == '"' && last == '"') || (first == '\'' && last == '\'') {
            return s[1..s.len() - 1].to_string();
        }
    }
    s.to_string()
}

/// Serialize a [`Frontmatter`] struct to a YAML string (without `---` delimiters).
pub fn serialize_frontmatter(fm: &Frontmatter) -> String {
    let mut lines: Vec<String> = Vec::new();

    if !fm.title.is_empty() {
        lines.push(format_yaml_scalar("title", &fm.title));
    }

    // Tags as inline list: tags: [tag1, tag2]
    if fm.tags.is_empty() {
        lines.push("tags: []".to_string());
    } else {
        let quoted: Vec<String> = fm.tags.iter().map(|t| quote_yaml_scalar(t)).collect();
        lines.push(format!("tags: [{}]", quoted.join(", ")));
    }

    if let Some(ref created) = fm.created {
        lines.push(format_yaml_scalar("created", created));
    }

    if let Some(ref modified) = fm.modified {
        lines.push(format_yaml_scalar("modified", modified));
    }

    for (key, val) in &fm.other {
        lines.push(format_yaml_scalar(key, val));
    }

    lines.join("\n")
}

/// Format a `key: value` line, quoting the value if needed.
fn format_yaml_scalar(key: &str, value: &str) -> String {
    format!("{}: {}", key, quote_yaml_scalar(value))
}

/// Quote a YAML scalar value if it contains special characters.
fn quote_yaml_scalar(s: &str) -> String {
    if s.is_empty() {
        return "\"\"".to_string();
    }

    // Quote if the value contains characters that could confuse YAML
    // Note: plain scalars CAN contain spaces — `title: My Note` is valid YAML.
    // We only quote when truly necessary (special indicators, reserved words).
    let needs_quote = s.contains(": ")
        || s.ends_with(':')
        || s.contains(" #")
        || s.starts_with('#')
        || s.contains('\n')
        || s.contains('\t')
        || s.starts_with('[')
        || s.starts_with(']')
        || s.starts_with('{')
        || s.starts_with('}')
        || s.starts_with(',')
        || s.starts_with('&')
        || s.starts_with('*')
        || s.starts_with('!')
        || s.starts_with('|')
        || s.starts_with('>')
        || s.starts_with('%')
        || s.starts_with('-')
        || s.starts_with('?')
        || s.starts_with("\"")
        || s.starts_with("'")
        || s.starts_with(' ')
        || s.ends_with(' ')
        || s.parse::<f64>().is_ok()
        || matches!(
            s,
            "true" | "false" | "null" | "~" | "True" | "False" | "Null" | "TRUE" | "FALSE" | "NULL"
        );

    if needs_quote {
        // Use double quotes and escape inner quotes
        let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{}\"", escaped)
    } else {
        s.to_string()
    }
}

/// Build a complete Markdown document with frontmatter.
///
/// The output format is:
/// ```markdown
/// ---
/// title: My Note
/// tags: [tag1, tag2]
/// created: 2024-01-01T00:00:00Z
/// modified: 2024-01-02T00:00:00Z
/// ---
///
/// body content...
/// ```
pub fn build_note(fm: &Frontmatter, body: &str) -> String {
    let yaml = serialize_frontmatter(fm);
    format!("{}\n{}\n---\n\n{}", FRONTMATTER_DELIM, yaml, body)
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_frontmatter_basic() {
        let md = "---\ntitle: Hello World\ntags: [a, b]\ncreated: 2024-01-01T00:00:00Z\nmodified: 2024-01-02T00:00:00Z\n---\n\nBody content";
        let parsed = parse_note(md);
        assert_eq!(parsed.frontmatter.title, "Hello World");
        assert_eq!(parsed.frontmatter.tags, vec!["a", "b"]);
        assert_eq!(
            parsed.frontmatter.created,
            Some("2024-01-01T00:00:00Z".into())
        );
        assert_eq!(
            parsed.frontmatter.modified,
            Some("2024-01-02T00:00:00Z".into())
        );
        assert_eq!(parsed.content.trim(), "Body content");
    }

    #[test]
    fn test_parse_frontmatter_block_list() {
        let md = "---\ntitle: Test\ntags:\n  - apple\n  - banana\n---\n\nContent";
        let parsed = parse_note(md);
        assert_eq!(parsed.frontmatter.title, "Test");
        assert_eq!(parsed.frontmatter.tags, vec!["apple", "banana"]);
    }

    #[test]
    fn test_parse_frontmatter_no_frontmatter() {
        let md = "# Just a heading\n\nSome content";
        let parsed = parse_note(md);
        assert_eq!(parsed.frontmatter.title, "");
        assert!(parsed.frontmatter.tags.is_empty());
        assert_eq!(parsed.content.trim(), "# Just a heading\n\nSome content");
    }

    #[test]
    fn test_parse_frontmatter_empty_tags() {
        let md = "---\ntitle: No Tags\ntags: []\n---\n\nHello";
        let parsed = parse_note(md);
        assert_eq!(parsed.frontmatter.title, "No Tags");
        assert!(parsed.frontmatter.tags.is_empty());
    }

    #[test]
    fn test_parse_frontmatter_quoted_values() {
        let md = "---\ntitle: \"Hello: World\"\ncreated: '2024-01-01'\n---\n";
        let parsed = parse_note(md);
        assert_eq!(parsed.frontmatter.title, "Hello: World");
        assert_eq!(parsed.frontmatter.created, Some("2024-01-01".into()));
    }

    #[test]
    fn test_serialize_frontmatter_roundtrip() {
        let fm = Frontmatter {
            title: "My Note".into(),
            tags: vec!["tag1".into(), "tag2".into()],
            created: Some("2024-01-01T00:00:00Z".into()),
            modified: Some("2024-01-02T00:00:00Z".into()),
            ..Default::default()
        };
        let built = build_note(&fm, "Hello world");
        let parsed = parse_note(&built);
        assert_eq!(parsed.frontmatter.title, "My Note");
        assert_eq!(parsed.frontmatter.tags, vec!["tag1", "tag2"]);
        assert_eq!(
            parsed.frontmatter.created,
            Some("2024-01-01T00:00:00Z".into())
        );
        assert_eq!(
            parsed.frontmatter.modified,
            Some("2024-01-02T00:00:00Z".into())
        );
        assert_eq!(parsed.content, "Hello world");
    }

    #[test]
    fn test_quote_yaml_scalar() {
        assert_eq!(quote_yaml_scalar("simple"), "simple");
        assert_eq!(quote_yaml_scalar("has:colon"), "has:colon");
        assert_eq!(quote_yaml_scalar("true"), "\"true\"");
        assert_eq!(quote_yaml_scalar("42"), "\"42\"");
        assert_eq!(quote_yaml_scalar("tag with space"), "tag with space");
        assert_eq!(quote_yaml_scalar("Hello: World"), "\"Hello: World\"");
        assert_eq!(quote_yaml_scalar(""), "\"\"");
    }
}
