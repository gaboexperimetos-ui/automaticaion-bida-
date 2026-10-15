//! Wikilink parsing and resolution for Obsidian-compatible Markdown.
//!
//! Wikilinks are Markdown-style links written as `[[Note Name]]`. They are
//! the core linking primitive in Obsidian's "second brain" graph. This module
//! parses them from note content and provides slug-based resolution.

/// A wikilink parsed from Markdown content: `[[Note Name]]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wikilink {
    /// The target note name (e.g. `"Meeting Notes"`).
    pub target: String,
    /// Optional display text after `|` (e.g. `[[Note|Alias]]`).
    /// If absent the target itself is the display text.
    pub display: Option<String>,
    /// Optional heading anchor after `#` (e.g. `[[Note#Section]]`).
    pub heading: Option<String>,
    /// Byte offset of the opening `[[` in the source content.
    pub offset: usize,
}

impl Wikilink {
    /// Resolve the wikilink target to a slug for matching against `notes.slug`.
    ///
    /// In Obsidian, `[[Meeting Notes]]` links to a file named `Meeting Notes.md`.
    /// Our slug system converts spaces to hyphens, so the slug is
    /// `"meeting-notes"`.
    pub fn resolve(&self) -> String {
        crate::models::Note::slugify(&self.target)
    }
}

/// Parse all wikilinks from Markdown content.
///
/// Supports the four Obsidian wikilink forms:
/// - `[[Note Name]]`
/// - `[[Note Name|Display Text]]`
/// - `[[Note Name#Heading]]`
/// - `[[Note Name#Heading|Display Text]]`
///
/// Returns wikilinks in document order with byte offsets.
///
/// # Examples
/// ```
/// use study_platform::knowledge::wikilinks::parse_wikilinks;
///
/// let content = "See [[My Note]] and [[Other|alias]] and [[Page#Section|display]].";
/// let links = parse_wikilinks(content);
/// assert_eq!(links.len(), 3);
/// assert_eq!(links[0].target, "My Note");
/// assert_eq!(links[0].resolve(), "my-note");
/// assert_eq!(links[1].target, "Other");
/// assert_eq!(links[1].display, Some("alias".into()));
/// assert_eq!(links[2].target, "Page");
/// assert_eq!(links[2].heading, Some("Section".into()));
/// assert_eq!(links[2].display, Some("display".into()));
/// ```
pub fn parse_wikilinks(content: &str) -> Vec<Wikilink> {
    let mut links = Vec::new();
    let bytes = content.as_bytes();
    let mut i = 0usize;

    while i + 1 < bytes.len() {
        // Look for `[[`
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            let start = i + 2; // position after `[[`

            if let Some(close_pos) = find_double_close(bytes, start) {
                let inner = &content[start..close_pos];
                if let Some(link) = parse_wikilink_inner(inner, i) {
                    links.push(link);
                }
                i = close_pos + 2; // skip past `]]`
            } else {
                // Unclosed `[[` — skip the two brackets
                i += 2;
            }
        } else {
            i += 1;
        }
    }

    links
}

/// Find the position of the first `]]` at or after `start` in `bytes`.
fn find_double_close(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start;
    while i + 1 < bytes.len() {
        if bytes[i] == b']' && bytes[i + 1] == b']' {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Parse the inner content of a `[[...]]` wikilink.
///
/// Accepted forms:
/// - `target`
/// - `target|display`
/// - `target#heading`
/// - `target#heading|display`
fn parse_wikilink_inner(inner: &str, offset: usize) -> Option<Wikilink> {
    let trimmed = inner.trim();

    if trimmed.is_empty() {
        return None;
    }

    // Split on `|` to separate target from display text.
    // Note: headings can contain `|` in theory, but Obsidian doesn't support
    // that edge case — the first `|` is always the display separator.
    let (target_part, display) = match trimmed.find('|') {
        Some(pipe_pos) => {
            let target = trimmed[..pipe_pos].trim();
            let display_text = trimmed[pipe_pos + 1..].trim();
            (target, Some(display_text.to_string()))
        }
        None => (trimmed, None),
    };

    // Split target on `#` to separate target from heading.
    let (target, heading) = match target_part.find('#') {
        Some(hash_pos) => {
            let target = target_part[..hash_pos].trim();
            let heading_text = target_part[hash_pos + 1..].trim();
            (target.to_string(), Some(heading_text.to_string()))
        }
        None => (target_part.to_string(), None),
    };

    if target.is_empty() {
        return None;
    }

    Some(Wikilink {
        target,
        display,
        heading,
        offset,
    })
}

/// Extract unique, resolved slugs of all wikilinks found in `content`.
///
/// Useful for building a backlink graph or pre-validating links.
///
/// # Examples
/// ```
/// use study_platform::knowledge::wikilinks::extract_linked_slugs;
///
/// let content = "Links to [[My Note]] and [[My Note]] and [[Other Page]].";
/// let slugs = extract_linked_slugs(content);
/// assert_eq!(slugs, vec!["my-note".to_string(), "other-page".to_string()]);
/// ```
pub fn extract_linked_slugs(content: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();

    for link in parse_wikilinks(content) {
        let slug = link.resolve();
        if seen.insert(slug.clone()) {
            result.push(slug);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Note;

    #[test]
    fn test_parse_simple_wikilink() {
        let links = parse_wikilinks("See [[My Note]] here.");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "My Note");
        assert_eq!(links[0].display, None);
        assert_eq!(links[0].heading, None);
        assert_eq!(links[0].resolve(), "my-note");
    }

    #[test]
    fn test_parse_wikilink_with_display() {
        let links = parse_wikilinks("[[Other Page|alias text]]");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "Other Page");
        assert_eq!(links[0].display, Some("alias text".into()));
        assert_eq!(links[0].resolve(), "other-page");
    }

    #[test]
    fn test_parse_wikilink_with_heading() {
        let links = parse_wikilinks("[[Page#Section]]");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "Page");
        assert_eq!(links[0].heading, Some("Section".into()));
    }

    #[test]
    fn test_parse_wikilink_with_heading_and_display() {
        let links = parse_wikilinks("[[Page#Section|Display Text]]");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "Page");
        assert_eq!(links[0].heading, Some("Section".into()));
        assert_eq!(links[0].display, Some("Display Text".into()));
    }

    #[test]
    fn test_parse_multiple_wikilinks() {
        let content = "[[Note A]] and [[Note B|alias]] and [[Note C#hdr|disp]]";
        let links = parse_wikilinks(content);
        assert_eq!(links.len(), 3);
        assert_eq!(links[0].target, "Note A");
        assert_eq!(links[1].target, "Note B");
        assert_eq!(links[2].target, "Note C");
    }

    #[test]
    fn test_parse_no_wikilinks() {
        let content = "No links here, just regular [markdown](url) and brackets [like] these.";
        let links = parse_wikilinks(content);
        assert_eq!(links, vec![]);
    }

    #[test]
    fn test_parse_unclosed_wikilink() {
        let content = "[[Unclosed link";
        let links = parse_wikilinks(content);
        assert_eq!(links, vec![]);
    }

    #[test]
    fn test_parse_empty_wikilink() {
        // `[[]]` is a truly empty wikilink (empty target between `[[` and `]]`)
        let links = parse_wikilinks("[[]]");
        assert_eq!(links, vec![]);
    }

    #[test]
    fn test_parse_wikilink_offsets() {
        let content = "ab[[Link]]cd";
        let links = parse_wikilinks(content);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].offset, 2); // position of `[[`
    }

    #[test]
    fn test_extract_linked_slugs_unique() {
        let content = "[[My Note]] and [[My Note]] and [[Other]]";
        let slugs = extract_linked_slugs(content);
        assert_eq!(slugs, vec!["my-note".to_string(), "other".to_string()]);
    }

    #[test]
    fn test_slugify_variations() {
        assert_eq!(Note::slugify("Hello World"), "hello-world");
        assert_eq!(
            Note::slugify("Data Structures & Algorithms"),
            "data-structures--algorithms"
        );
        assert_eq!(Note::slugify("¡Hola! ¿Cómo?"), "hola-cómo");
        assert_eq!(Note::slugify("  Test  "), "test");
    }
}
