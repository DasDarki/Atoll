//! Reading Opera's bookmarks, which it keeps as the JSON file Chromium writes.

use std::path::Path;

use serde_json::Value;

use crate::BookmarkNode;
use crate::opera::OperaError;

/// Where Opera keeps bookmarks and what the folder is called here, in the order they are taken
/// over. Opera has the roots of Chromium and its own ones under `custom_root`. The trash and the
/// pinboards of unsynced notes are left behind.
const ROOTS: &[(&[&str], &str)] = &[
    (&["bookmark_bar"], "Lesezeichenleiste"),
    (&["custom_root", "userRoot"], "Lesezeichen"),
    (&["custom_root", "speedDial"], "Speed Dial"),
    (&["custom_root", "unsorted"], "Unsortiert"),
    (&["custom_root", "pinboard"], "Pinnwand"),
    (&["other"], "Andere Lesezeichen"),
    (&["synced"], "Synchronisiert"),
];

/// Reads the bookmarks of an Opera profile as a tree.
pub fn read_bookmarks(directory: &Path) -> Result<Vec<BookmarkNode>, OperaError> {
    let path = directory.join("Bookmarks");
    let text = std::fs::read_to_string(&path)
        .map_err(|error| OperaError::Unreadable(format!("Bookmarks: {error}")))?;
    let file: Value = serde_json::from_str(&text)
        .map_err(|error| OperaError::Unreadable(format!("Bookmarks: {error}")))?;
    let roots = file
        .get("roots")
        .ok_or_else(|| OperaError::Unreadable("Bookmarks: no roots".to_owned()))?;
    let mut nodes = Vec::new();
    for (path, fallback) in ROOTS {
        let Some(root) = path.iter().try_fold(roots, |value, key| value.get(key)) else {
            continue;
        };
        let children = node_list(root);
        if children.is_empty() {
            continue;
        }
        // The bar of the other browser becomes the bar here, the rest keeps its own folder.
        if path == &["bookmark_bar"] {
            nodes.extend(children);
        } else {
            nodes.push(BookmarkNode::Folder {
                title: title_of(root, fallback),
                children,
            });
        }
    }
    Ok(nodes)
}

/// The children of a folder, as far as they are bookmarks or folders.
fn node_list(folder: &Value) -> Vec<BookmarkNode> {
    folder
        .get("children")
        .and_then(Value::as_array)
        .map(|children| children.iter().filter_map(node).collect())
        .unwrap_or_default()
}

fn node(value: &Value) -> Option<BookmarkNode> {
    match value.get("type").and_then(Value::as_str)? {
        "url" => {
            let url = value.get("url").and_then(Value::as_str)?;
            if !url.starts_with("http://") && !url.starts_with("https://") {
                return None;
            }
            Some(BookmarkNode::Link {
                title: title_of(value, url),
                url: url.to_owned(),
            })
        }
        "folder" => {
            let children = node_list(value);
            if children.is_empty() {
                return None;
            }
            Some(BookmarkNode::Folder {
                title: title_of(value, "Ordner"),
                children,
            })
        }
        _ => None,
    }
}

fn title_of(value: &Value, fallback: &str) -> String {
    value
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(fallback)
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = r#"{
      "roots": {
        "bookmark_bar": {
          "name": "Lesezeichenleiste",
          "type": "folder",
          "children": [
            { "type": "url", "name": "Erste", "url": "https://a.example/" },
            { "type": "folder", "name": "Arbeit", "children": [
                { "type": "url", "name": "Zweite", "url": "https://b.example/" },
                { "type": "url", "name": "Opera", "url": "opera://startpage" }
            ] }
          ]
        },
        "other": {
          "name": "Andere Lesezeichen",
          "type": "folder",
          "children": [ { "type": "url", "name": "", "url": "https://c.example/" } ]
        },
        "custom_root": {
          "userRoot": {
            "name": "Lesezeichen",
            "type": "folder",
            "children": [ { "type": "url", "name": "Eigene", "url": "https://e.example/" } ]
          },
          "trash": {
            "name": "Papierkorb",
            "type": "folder",
            "children": [ { "type": "url", "name": "Weg", "url": "https://f.example/" } ]
          }
        },
        "trash": {
          "name": "Papierkorb",
          "type": "folder",
          "children": [ { "type": "url", "name": "Weg", "url": "https://d.example/" } ]
        }
      }
    }"#;

    fn scratch(name: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "atoll-opera-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn the_bar_stays_the_bar_and_the_rest_keeps_its_folder() {
        let directory = scratch("bookmarks");
        std::fs::write(directory.join("Bookmarks"), FILE).unwrap();

        let nodes = read_bookmarks(&directory).unwrap();
        assert_eq!(nodes.len(), 4, "two from the bar and two folders");
        assert_eq!(
            nodes[0],
            BookmarkNode::Link {
                title: "Erste".to_owned(),
                url: "https://a.example/".to_owned(),
            }
        );
        let BookmarkNode::Folder { title, children } = &nodes[1] else {
            panic!("the folder of the bar is kept");
        };
        assert_eq!(title, "Arbeit");
        assert_eq!(children.len(), 1, "pages of Opera itself are left behind");
        assert!(
            !format!("{nodes:?}").contains("f.example"),
            "the trash stays behind"
        );
        let BookmarkNode::Folder { title, .. } = &nodes[2] else {
            panic!("Opera's own bookmarks keep their folder");
        };
        assert_eq!(title, "Lesezeichen");
        let BookmarkNode::Folder { title, children } = &nodes[3] else {
            panic!("the other bookmarks keep their folder");
        };
        assert_eq!(title, "Andere Lesezeichen");
        assert_eq!(
            children[0],
            BookmarkNode::Link {
                title: "https://c.example/".to_owned(),
                url: "https://c.example/".to_owned(),
            },
            "a bookmark without a name is named after its address"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_profile_without_bookmarks_says_so() {
        let directory = scratch("empty");
        assert!(read_bookmarks(&directory).is_err());
        let _ = std::fs::remove_dir_all(&directory);
    }
}
