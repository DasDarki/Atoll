//! Checks that the translation files and the strings in the source say the same thing.
//!
//! The strings of Atoll are German and live in the source. Every language has a gettext file
//! under `lang/`, which Slint bundles into the user interface and `atoll-i18n` compiles into its
//! catalog. A string that is added without a translation would silently stay German, so the test
//! reads both sides and compares them. `scripts/translations.py update` fixes what it reports.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The languages with a file of their own. German is the language of the source.
const LANGUAGES: &[&str] = &["en"];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .to_path_buf()
}

/// Every file of a folder with that ending, folders included.
fn files(directory: &Path, ending: &str, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            files(&path, ending, found);
        } else if path.extension().is_some_and(|actual| actual == ending) {
            found.push(path);
        }
    }
}

/// The text of the literal that follows `start`, and the index behind it. Only whitespace may
/// stand between the two, so a call with something else in front of its text is passed over.
fn literal(text: &str, start: usize) -> Option<(String, usize)> {
    let rest = text.get(start..)?;
    let open = rest.find('"')?;
    if !rest[..open].trim().is_empty() {
        return None;
    }
    let mut result = String::new();
    let mut escape = false;
    for (offset, character) in rest[open + 1..].char_indices() {
        if escape {
            result.push(match character {
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                other => other,
            });
            escape = false;
        } else if character == '\\' {
            escape = true;
        } else if character == '"' {
            return Some((result, start + open + 1 + offset + 1));
        } else {
            result.push(character);
        }
    }
    None
}

/// Every string a call of `marker` asks to translate, for example `t!(` or `@tr(`. With `plural`
/// the second text of `@tr("one" | "many" % n)` is picked up as well.
fn strings_after(text: &str, marker: &str, plural: bool) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(offset) = text[at..].find(marker) {
        let start = at + offset;
        at = start + marker.len();
        if marker == "t!(" && start > 0 {
            let previous = bytes[start - 1];
            if previous.is_ascii_alphanumeric() || previous == b'_' || previous == b'!' {
                continue;
            }
        }
        let Some((one, end)) = literal(text, at) else {
            continue;
        };
        at = end;
        found.push(one);
        if plural && text[at..].trim_start().starts_with('|') {
            let bar = at + text[at..].find('|').unwrap_or_default();
            if let Some((many, end)) = literal(text, bar + 1) {
                at = end;
                found.push(many);
            }
        }
    }
    found
}

/// Every string of Atoll that has to be translated.
fn wanted() -> BTreeSet<String> {
    let root = root();
    let mut sources = Vec::new();
    files(&root.join("crates"), "rs", &mut sources);
    let mut found = BTreeSet::new();
    for path in sources {
        if path.components().any(|part| part.as_os_str() == "tests") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        found.extend(strings_after(&text, "t!(", false));
    }
    let mut interfaces = Vec::new();
    files(&root.join("crates/atoll/ui"), "slint", &mut interfaces);
    for path in interfaces {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        found.extend(strings_after(&text, "@tr(", true));
    }
    found
}

/// What a translation file offers, by the German original.
fn catalog(language: &str) -> Vec<(String, String)> {
    let path = root()
        .join("lang")
        .join(language)
        .join("LC_MESSAGES")
        .join("atoll.po");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} could not be read: {error}", path.display()));
    let mut entries = Vec::new();
    let mut originals: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("msgid ") {
            originals.clear();
            if let Some((original, _)) = literal(rest, 0) {
                originals.push(original);
            }
        } else if let Some(rest) = line.strip_prefix("msgid_plural ")
            && let Some((original, _)) = literal(rest, 0)
        {
            originals.push(original);
        } else if let Some(rest) = line
            .strip_prefix("msgstr ")
            .or_else(|| line.strip_prefix("msgstr[0] "))
            && let Some((translation, _)) = literal(rest, 0)
        {
            if let Some(original) = originals.first() {
                entries.push((original.clone(), translation));
            }
        } else if let Some(rest) = line.strip_prefix("msgstr[1] ")
            && let Some((translation, _)) = literal(rest, 0)
            && let Some(original) = originals.get(1)
        {
            entries.push((original.clone(), translation));
        }
    }
    entries
}

#[test]
fn every_string_of_atoll_is_translated() {
    let wanted = wanted();
    assert!(
        wanted.len() > 300,
        "the strings of Atoll were not found, only {} came out",
        wanted.len()
    );
    for language in LANGUAGES {
        let catalog = catalog(language);
        let translated: BTreeSet<&str> = catalog
            .iter()
            .filter(|(_, translation)| !translation.is_empty())
            .map(|(original, _)| original.as_str())
            .collect();
        let missing: Vec<&String> = wanted
            .iter()
            .filter(|text| !translated.contains(text.as_str()))
            .collect();
        assert!(
            missing.is_empty(),
            "{language} is missing {} of {} strings, run scripts/translations.py update: {:?}",
            missing.len(),
            wanted.len(),
            &missing[..missing.len().min(10)]
        );
    }
}

#[test]
fn no_translation_is_left_over() {
    let wanted = wanted();
    for language in LANGUAGES {
        let stale: Vec<String> = catalog(language)
            .into_iter()
            .map(|(original, _)| original)
            .filter(|original| !original.is_empty() && !wanted.contains(original))
            .collect();
        assert!(
            stale.is_empty(),
            "{language} translates {} strings Atoll no longer has: {:?}",
            stale.len(),
            &stale[..stale.len().min(10)]
        );
    }
}
