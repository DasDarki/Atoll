use std::path::PathBuf;

pub(crate) const PROGRESS_MARK: &str = "ATOLL-PROGRESS";
pub(crate) const NAME_MARK: &str = "ATOLL-NAME";
pub(crate) const FILE_MARK: &str = "ATOLL-FILE";

/// How far the current file of a download is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Progress {
    pub received_bytes: i64,
    /// The expected size, zero while it is unknown.
    pub total_bytes: i64,
    /// Bytes per second.
    pub speed: i64,
}

/// A line of yt-dlp's output that Atoll asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Line {
    Progress(Progress),
    Name(PathBuf),
    File(PathBuf),
}

pub(crate) fn parse_line(line: &str) -> Option<Line> {
    let (mark, rest) = line.trim_end().split_once(' ')?;
    match mark {
        PROGRESS_MARK => {
            let mut values = rest.split(' ').map(number);
            let received_bytes = values.next()??;
            let total = values.next()?;
            let estimate = values.next()?;
            let speed = values.next()?;
            Some(Line::Progress(Progress {
                received_bytes,
                total_bytes: total.or(estimate).unwrap_or(0),
                speed: speed.unwrap_or(0),
            }))
        }
        NAME_MARK => Some(Line::Name(PathBuf::from(rest))),
        FILE_MARK => Some(Line::File(PathBuf::from(rest))),
        _ => None,
    }
}

/// A number yt-dlp printed, `None` for `NA`.
fn number(text: &str) -> Option<i64> {
    text.parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value >= 0.0)
        .map(|value| value as i64)
}

/// The message of an error line on yt-dlp's error output, without the extractor prefix.
pub(crate) fn error_message(line: &str) -> Option<String> {
    let message = line.trim().strip_prefix("ERROR:")?.trim();
    let message = match message.strip_prefix('[') {
        Some(rest) => rest.split_once("] ").map_or(message, |(_, rest)| rest),
        None => message,
    };
    Some(message.to_owned())
}

/// `name` as a yt-dlp output template: without characters file systems reject and with `%`
/// escaped, at most 150 characters long.
pub(crate) fn output_template(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => ' ',
            character if character.is_control() => ' ',
            character => character,
        })
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let cleaned: String = cleaned.trim_matches('.').chars().take(150).collect();
    let cleaned = if cleaned.trim().is_empty() {
        "Video".to_owned()
    } else {
        cleaned.trim().to_owned()
    };
    format!("{}.%(ext)s", cleaned.replace('%', "%%"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_prefers_the_exact_size_over_the_estimate() {
        assert_eq!(
            parse_line("ATOLL-PROGRESS 1024 NA 231240.0 0"),
            Some(Line::Progress(Progress {
                received_bytes: 1024,
                total_bytes: 231240,
                speed: 0,
            }))
        );
        assert_eq!(
            parse_line("ATOLL-PROGRESS 231992 231992 NA 14157911.96\n"),
            Some(Line::Progress(Progress {
                received_bytes: 231992,
                total_bytes: 231992,
                speed: 14157911,
            }))
        );
        assert_eq!(
            parse_line("ATOLL-PROGRESS NA NA NA NA"),
            None,
            "without received bytes there is nothing to show"
        );
    }

    #[test]
    fn names_and_files_keep_spaces() {
        assert_eq!(
            parse_line("ATOLL-NAME /home/kim/Downloads/Mein 50% Video.mp4"),
            Some(Line::Name(PathBuf::from(
                "/home/kim/Downloads/Mein 50% Video.mp4"
            )))
        );
        assert_eq!(
            parse_line("ATOLL-FILE /tmp/a b.webm"),
            Some(Line::File(PathBuf::from("/tmp/a b.webm")))
        );
        assert_eq!(parse_line("[download] Destination: x.mp4"), None);
        assert_eq!(parse_line("ATOLL-PROGRESS"), None);
    }

    #[test]
    fn error_messages_lose_the_extractor_prefix() {
        assert_eq!(
            error_message("ERROR: [generic] missing: Unable to download webpage: HTTP Error 404"),
            Some("missing: Unable to download webpage: HTTP Error 404".to_owned())
        );
        assert_eq!(
            error_message("ERROR: Unsupported URL: http://127.0.0.1:8766/"),
            Some("Unsupported URL: http://127.0.0.1:8766/".to_owned())
        );
        assert_eq!(error_message("WARNING: falling back"), None);
    }

    #[test]
    fn output_templates_are_safe_file_names() {
        assert_eq!(output_template("Mein 50% Video"), "Mein 50%% Video.%(ext)s");
        assert_eq!(
            output_template("r/videos: a/b \"c\"\n"),
            "r videos a b c.%(ext)s"
        );
        assert_eq!(output_template(" .. "), "Video.%(ext)s");
        assert_eq!(
            output_template(&"x".repeat(300)).len(),
            150 + ".%(ext)s".len()
        );
    }
}
