//! Turns the translation files into a table the crate can look up without reading files at
//! startup. The same files feed Slint's bundled translations, so there is one place to translate.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The languages Atoll is translated into. German is the language of the source strings and needs
/// no file of its own.
const LANGUAGES: &[&str] = &["en"];

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("..")
        .join("..")
        .join("lang");
    let mut catalogs = String::new();
    let mut lookup = String::new();
    for language in LANGUAGES {
        let path = root.join(language).join("LC_MESSAGES").join("atoll.po");
        println!("cargo:rerun-if-changed={}", path.display());
        let entries = read_catalog(&path);
        catalogs.push_str(&format!(
            "static {}: &[(&str, &str)] = &[\n",
            language.to_uppercase()
        ));
        for (id, text) in &entries {
            catalogs.push_str(&format!("    ({}, {}),\n", quote(id), quote(text)));
        }
        catalogs.push_str("];\n\n");
        lookup.push_str(&format!(
            "        Language::{} => {},\n",
            variant(language),
            language.to_uppercase()
        ));
    }
    let source = format!(
        "{catalogs}/// The translations of `language`, sorted by the German original.\n\
         fn catalog(language: Language) -> &'static [(&'static str, &'static str)] {{\n    \
         match language {{\n        Language::German => &[],\n{lookup}    }}\n}}\n"
    );
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("catalog.rs");
    std::fs::write(&out, source).unwrap();
}

fn variant(language: &str) -> &str {
    match language {
        "en" => "English",
        other => panic!("{other} is not a language this crate knows"),
    }
}

/// Reads a gettext file into the pairs the catalog is built from. Entries without a translation
/// are left out, so the German original is used for them.
fn read_catalog(path: &Path) -> BTreeMap<String, String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    let mut entries = BTreeMap::new();
    let mut id: Option<String> = None;
    let mut translation: Option<String> = None;
    let mut collecting = Collecting::Nothing;
    let mut finish = |id: &mut Option<String>, translation: &mut Option<String>| {
        if let (Some(id), Some(translation)) = (id.take(), translation.take())
            && !id.is_empty()
            && !translation.is_empty()
        {
            entries.insert(id, translation);
        }
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            finish(&mut id, &mut translation);
            collecting = Collecting::Nothing;
            continue;
        }
        if let Some(rest) = line.strip_prefix("msgid ") {
            finish(&mut id, &mut translation);
            id = Some(unquote(rest));
            collecting = Collecting::Id;
        } else if let Some(rest) = line.strip_prefix("msgstr ") {
            translation = Some(unquote(rest));
            collecting = Collecting::Translation;
        } else if line.starts_with('"') {
            let more = unquote(line);
            match collecting {
                Collecting::Id => id.get_or_insert_default().push_str(&more),
                Collecting::Translation => translation.get_or_insert_default().push_str(&more),
                Collecting::Nothing => {}
            }
        } else {
            finish(&mut id, &mut translation);
            collecting = Collecting::Nothing;
        }
    }
    finish(&mut id, &mut translation);
    entries
}

enum Collecting {
    Nothing,
    Id,
    Translation,
}

/// The text of one `"..."` of a gettext file.
fn unquote(text: &str) -> String {
    let inner = text
        .trim()
        .trim_start_matches('"')
        .trim_end_matches('"')
        .to_owned();
    let mut result = String::with_capacity(inner.len());
    let mut characters = inner.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            result.push(character);
            continue;
        }
        match characters.next() {
            Some('n') => result.push('\n'),
            Some('t') => result.push('\t'),
            Some('r') => result.push('\r'),
            Some(other) => result.push(other),
            None => {}
        }
    }
    result
}

fn quote(text: &str) -> String {
    format!("{text:?}")
}
