/// A named group of addresses to export, usually one island.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportGroup {
    pub name: String,
    pub urls: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportOptions {
    /// Writes `--- NAME ---` markers. Without them the result is a plain list, one URL per line.
    pub island_markers: bool,
}

/// Serialises groups into the URL list format. Blank addresses are skipped.
pub fn export(groups: &[ExportGroup], options: ExportOptions) -> String {
    let mut output = String::new();
    for group in groups {
        let urls: Vec<&str> = group
            .urls
            .iter()
            .map(|url| url.trim())
            .filter(|url| !url.is_empty())
            .collect();

        if options.island_markers {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(&format!("--- {} ---\n", marker_name(&group.name)));
        }
        for url in urls {
            output.push_str(url);
            output.push('\n');
        }
    }
    output
}

fn marker_name(name: &str) -> String {
    let single_line: String = name
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();
    let trimmed = single_line.trim();
    if trimmed.is_empty() {
        String::from("Unbenannt")
    } else {
        trimmed.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    fn groups() -> Vec<ExportGroup> {
        vec![
            ExportGroup {
                name: "IWMedia".into(),
                urls: vec!["https://example.com/dashboard".into(), "".into()],
            },
            ExportGroup {
                name: " \n".into(),
                urls: vec!["https://example.org/".into()],
            },
        ]
    }

    #[test]
    fn writes_markers_and_blank_lines_between_islands() {
        let text = export(
            &groups(),
            ExportOptions {
                island_markers: true,
            },
        );
        assert_eq!(
            text,
            "--- IWMedia ---\nhttps://example.com/dashboard\n\n--- Unbenannt ---\nhttps://example.org/\n"
        );
    }

    #[test]
    fn plain_export_is_one_url_per_line() {
        let text = export(
            &groups(),
            ExportOptions {
                island_markers: false,
            },
        );
        assert_eq!(
            text,
            "https://example.com/dashboard\nhttps://example.org/\n"
        );
    }

    #[test]
    fn export_and_parse_round_trip() {
        let text = export(
            &groups(),
            ExportOptions {
                island_markers: true,
            },
        );
        let list = parse(&text);
        assert!(list.invalid.is_empty());
        assert_eq!(list.groups[0].name.as_deref(), Some("IWMedia"));
        assert_eq!(list.groups[1].name.as_deref(), Some("Unbenannt"));
        assert_eq!(list.url_count(), 2);
    }
}
