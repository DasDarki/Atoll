/// A node of an imported bookmark tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BookmarkNode {
    Folder {
        title: String,
        children: Vec<BookmarkNode>,
    },
    Link {
        title: String,
        url: String,
    },
}

impl BookmarkNode {
    /// Number of links in this node and below.
    pub fn link_count(&self) -> usize {
        match self {
            BookmarkNode::Folder { children, .. } => children.iter().map(Self::link_count).sum(),
            BookmarkNode::Link { .. } => 1,
        }
    }
}

/// Parses the bookmark HTML files exported by Opera, Chrome, Firefox and other browsers.
///
/// The browser's own bookmarks bar folder is unwrapped, its entries become top level entries.
pub fn parse_bookmarks_html(html: &str) -> Vec<BookmarkNode> {
    let mut stack: Vec<(Option<PendingFolder>, Vec<BookmarkNode>)> = vec![(None, Vec::new())];
    let mut pending: Option<PendingFolder> = None;
    let mut rest = html;

    while let Some(start) = rest.find('<') {
        let Some(end) = rest[start..].find('>').map(|end| start + end) else {
            break;
        };
        let tag = &rest[start + 1..end];
        let name = tag
            .split(|character: char| character.is_whitespace() || character == '/')
            .find(|part| !part.is_empty())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let closing = tag.starts_with('/');
        rest = &rest[end + 1..];

        match (name.as_str(), closing) {
            ("h3", false) => {
                let (title, remaining) = text_until(rest, "</h3");
                pending = Some(PendingFolder {
                    title,
                    toolbar: attribute(tag, "personal_toolbar_folder")
                        .is_some_and(|value| value == "true"),
                });
                rest = remaining;
            }
            ("a", false) => {
                let (title, remaining) = text_until(rest, "</a");
                rest = remaining;
                if let Some(url) = attribute(tag, "href").filter(|url| !url.trim().is_empty()) {
                    let title = if title.is_empty() { url.clone() } else { title };
                    if let Some((_, children)) = stack.last_mut() {
                        children.push(BookmarkNode::Link { title, url });
                    }
                }
            }
            ("dl", false) => stack.push((pending.take(), Vec::new())),
            ("dl", true) if stack.len() > 1 => {
                let (folder, children) = stack.pop().unwrap_or_default();
                let top_level = stack.len() <= 2;
                let Some((_, parent)) = stack.last_mut() else {
                    break;
                };
                match folder {
                    Some(folder) if folder.toolbar && top_level => parent.extend(children),
                    Some(folder) => parent.push(BookmarkNode::Folder {
                        title: folder.title,
                        children,
                    }),
                    None => parent.extend(children),
                }
            }
            _ => {}
        }
    }

    while stack.len() > 1 {
        let (folder, children) = stack.pop().unwrap_or_default();
        if let Some((_, parent)) = stack.last_mut() {
            match folder {
                Some(folder) => parent.push(BookmarkNode::Folder {
                    title: folder.title,
                    children,
                }),
                None => parent.extend(children),
            }
        }
    }
    stack
        .pop()
        .map(|(_, children)| children)
        .unwrap_or_default()
}

#[derive(Debug, Default)]
struct PendingFolder {
    title: String,
    toolbar: bool,
}

/// Returns the decoded text before the closing tag and the input after it.
fn text_until<'a>(input: &'a str, closing: &str) -> (String, &'a str) {
    let lower = input.to_ascii_lowercase();
    match lower.find(closing) {
        Some(position) => {
            let after = input[position..]
                .find('>')
                .map_or(input.len(), |end| position + end + 1);
            (decode_entities(input[..position].trim()), &input[after..])
        }
        None => (decode_entities(input.trim()), ""),
    }
}

fn attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut search = 0;
    while let Some(found) = lower[search..].find(name) {
        let position = search + found;
        search = position + name.len();
        let before = lower[..position].chars().last();
        if before.is_some_and(|character| !character.is_whitespace()) {
            continue;
        }
        let after = lower[search..].trim_start();
        let Some(value) = after.strip_prefix('=') else {
            continue;
        };
        let offset = tag.len() - value.len();
        let value = tag[offset..].trim_start();
        let parsed = match value.chars().next() {
            Some(quote @ ('"' | '\'')) => value[1..].split(quote).next().unwrap_or_default(),
            _ => value.split_whitespace().next().unwrap_or_default(),
        };
        return Some(decode_entities(parsed));
    }
    None
}

fn decode_entities(text: &str) -> String {
    let mut decoded = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        decoded.push_str(&rest[..start]);
        let entity_end = rest[start..].find(';').map(|end| start + end);
        let entity = entity_end.map(|end| &rest[start + 1..end]);
        let replacement = entity.and_then(|entity| match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            numeric => numeric
                .strip_prefix("#x")
                .or_else(|| numeric.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| {
                    numeric
                        .strip_prefix('#')
                        .and_then(|decimal| decimal.parse().ok())
                })
                .and_then(char::from_u32),
        });
        match (replacement, entity_end) {
            (Some(character), Some(end)) => {
                decoded.push(character);
                rest = &rest[end + 1..];
            }
            _ => {
                decoded.push('&');
                rest = &rest[start + 1..];
            }
        }
    }
    decoded.push_str(rest);
    decoded
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPORT: &str = r#"<!DOCTYPE NETSCAPE-Bookmark-file-1>
<META HTTP-EQUIV="Content-Type" CONTENT="text/html; charset=UTF-8">
<TITLE>Bookmarks</TITLE>
<H1>Bookmarks</H1>
<DL><p>
    <DT><H3 ADD_DATE="1700000000" PERSONAL_TOOLBAR_FOLDER="true">Lesezeichenleiste</H3>
    <DL><p>
        <DT><A HREF="https://github.com/" ADD_DATE="1700000001">GitHub</A>
        <DT><H3>Rust &amp; Co</H3>
        <DL><p>
            <DT><A HREF="https://doc.rust-lang.org/book/">The Rust Book</A>
        </DL><p>
    </DL><p>
    <DT><H3>Weitere Lesezeichen</H3>
    <DL><p>
        <DT><A HREF='https://example.com/?a=1&amp;b=2'></A>
        <DT><A>ohne Adresse</A>
    </DL><p>
</DL><p>"#;

    #[test]
    fn toolbar_folder_is_unwrapped_and_other_folders_kept() {
        let nodes = parse_bookmarks_html(EXPORT);
        assert_eq!(nodes.len(), 3);
        assert_eq!(
            nodes[0],
            BookmarkNode::Link {
                title: "GitHub".into(),
                url: "https://github.com/".into()
            }
        );
        let BookmarkNode::Folder { title, children } = &nodes[1] else {
            panic!("folder expected")
        };
        assert_eq!(title, "Rust & Co");
        assert_eq!(children.len(), 1);
        let BookmarkNode::Folder { title, children } = &nodes[2] else {
            panic!("folder expected")
        };
        assert_eq!(title, "Weitere Lesezeichen");
        assert_eq!(
            children[0],
            BookmarkNode::Link {
                title: "https://example.com/?a=1&b=2".into(),
                url: "https://example.com/?a=1&b=2".into()
            }
        );
    }

    #[test]
    fn links_are_counted_recursively() {
        let nodes = parse_bookmarks_html(EXPORT);
        assert_eq!(nodes.iter().map(BookmarkNode::link_count).sum::<usize>(), 3);
    }

    #[test]
    fn entities_are_decoded() {
        assert_eq!(
            decode_entities("a &lt;b&gt; &#252; &#xFC; &unknown; &"),
            "a <b> ü ü &unknown; &"
        );
    }

    #[test]
    fn unterminated_input_does_not_lose_links() {
        let nodes = parse_bookmarks_html(
            "<DL><DT><H3>Ordner</H3><DL><DT><A HREF=\"https://a.example/\">A</A>",
        );
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].link_count(), 1);
    }
}
