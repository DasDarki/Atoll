use crate::address::{InvalidReason, validate_address};

/// The result of parsing a URL list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedList {
    pub groups: Vec<ParsedGroup>,
    pub invalid: Vec<InvalidLine>,
}

impl ParsedList {
    pub fn url_count(&self) -> usize {
        self.groups.iter().map(|group| group.urls.len()).sum()
    }
}

/// Addresses that belong together. `name` is `None` for addresses before the first marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedGroup {
    pub name: Option<String>,
    pub urls: Vec<String>,
}

/// A line that could not be imported, reported to the user instead of being dropped silently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidLine {
    pub line_number: usize,
    pub content: String,
    pub reason: InvalidReason,
}

/// Parses a URL list. Markers of the form `--- NAME ---` start a new group.
pub fn parse(text: &str) -> ParsedList {
    let mut list = ParsedList::default();
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);

    for (index, raw_line) in text.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some(name) = island_marker(line) {
            list.groups.push(ParsedGroup {
                name: Some(name).filter(|name| !name.is_empty()),
                urls: Vec::new(),
            });
            continue;
        }

        match validate_address(line) {
            Ok(url) => current_group(&mut list).urls.push(url),
            Err(reason) => list.invalid.push(InvalidLine {
                line_number: index + 1,
                content: line.to_owned(),
                reason,
            }),
        }
    }

    list
}

fn island_marker(line: &str) -> Option<String> {
    let inner = line.strip_prefix("---")?.strip_suffix("---")?;
    if line.len() < 6 {
        return None;
    }
    Some(inner.trim().to_owned())
}

fn current_group(list: &mut ParsedList) -> &mut ParsedGroup {
    if list.groups.is_empty() {
        list.groups.push(ParsedGroup {
            name: None,
            urls: Vec::new(),
        });
    }
    list.groups.last_mut().expect("a group was just ensured")
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = "--- IWMedia ---\nhttps://example.com/dashboard\nhttps://example.com/tickets\n\n--- Recherche ---\nhttps://en.wikipedia.org/wiki/Web_browser\nhttps://github.com/brave/adblock-rust\n\n# Kommentarzeilen werden ignoriert\nhttps://example.org/ohne-insel\n";

    #[test]
    fn parses_the_documented_example() {
        let list = parse(EXAMPLE);
        assert!(list.invalid.is_empty());
        assert_eq!(list.groups.len(), 2);
        assert_eq!(list.groups[0].name.as_deref(), Some("IWMedia"));
        assert_eq!(list.groups[0].urls.len(), 2);
        assert_eq!(list.groups[1].name.as_deref(), Some("Recherche"));
        assert_eq!(
            list.groups[1].urls,
            [
                "https://en.wikipedia.org/wiki/Web_browser",
                "https://github.com/brave/adblock-rust",
                "https://example.org/ohne-insel"
            ]
        );
    }

    #[test]
    fn urls_before_the_first_marker_have_no_group_name() {
        let list = parse("https://a.example\n--- B ---\nhttps://b.example\n");
        assert_eq!(list.groups[0].name, None);
        assert_eq!(list.groups[0].urls, ["https://a.example/"]);
        assert_eq!(list.groups[1].name.as_deref(), Some("B"));
    }

    #[test]
    fn plain_lists_without_markers_form_one_unnamed_group() {
        let list = parse("https://a.example\r\n  https://b.example  \r\n");
        assert_eq!(list.groups.len(), 1);
        assert_eq!(list.url_count(), 2);
    }

    #[test]
    fn invalid_lines_are_reported_with_their_line_number() {
        let list = parse("--- A ---\nhttps://a.example\nkein link\nopera://settings\n");
        assert_eq!(list.url_count(), 1);
        assert_eq!(list.invalid.len(), 2);
        assert_eq!(list.invalid[0].line_number, 3);
        assert_eq!(list.invalid[0].content, "kein link");
        assert_eq!(
            list.invalid[1].reason,
            InvalidReason::UnsupportedScheme("opera".into())
        );
    }

    #[test]
    fn markers_keep_inner_dashes_and_empty_islands() {
        let list = parse("--- A --- B ---\n--- Leer ---\n---  ---\nhttps://c.example\n");
        let names: Vec<_> = list.groups.iter().map(|group| group.name.clone()).collect();
        assert_eq!(names, [Some("A --- B".into()), Some("Leer".into()), None]);
        assert_eq!(list.groups[2].urls, ["https://c.example/"]);
    }

    #[test]
    fn a_bare_triple_dash_is_not_a_marker() {
        let list = parse("---\n");
        assert!(list.groups.is_empty());
        assert_eq!(list.invalid.len(), 1);
    }

    #[test]
    fn byte_order_mark_is_ignored() {
        let list = parse("\u{feff}--- A ---\nhttps://a.example\n");
        assert_eq!(list.groups[0].name.as_deref(), Some("A"));
    }
}
