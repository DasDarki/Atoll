//! The languages Atoll speaks.
//!
//! The strings in the source are German, translations live in `lang/<language>/LC_MESSAGES` as
//! gettext files and are compiled into the binary. [`t!`] looks a string up in the language that
//! is set, and falls back to the German original where a translation is missing. The same files
//! are bundled into the user interface by Slint, so there is one place to translate.

use std::fmt::Display;
use std::sync::atomic::{AtomicU8, Ordering};

include!(concat!(env!("OUT_DIR"), "/catalog.rs"));

/// A language Atoll can be used in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Language {
    #[default]
    German,
    English,
}

impl Language {
    /// Every language, in the order the settings show them.
    pub const ALL: &'static [Language] = &[Language::German, Language::English];

    /// The short name it is stored and looked up under.
    pub fn tag(self) -> &'static str {
        match self {
            Language::German => "de",
            Language::English => "en",
        }
    }

    /// What the language calls itself, for the settings.
    pub fn label(self) -> &'static str {
        match self {
            Language::German => "Deutsch",
            Language::English => "English",
        }
    }

    /// The name Slint knows the bundled translation by. German is the language of the source
    /// strings, which Slint selects with an empty name.
    pub fn bundle(self) -> &'static str {
        match self {
            Language::German => "",
            Language::English => "en",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Language::ALL
            .iter()
            .copied()
            .find(|language| language.tag() == text)
    }

    /// The language of a locale like `de_DE.UTF-8`. Everything Atoll does not speak becomes
    /// English, which more people read than German.
    pub fn of_locale(locale: &str) -> Self {
        let base = locale
            .split(['_', '-', '.', '@'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        Language::parse(&base).unwrap_or(Language::English)
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

/// The language Atoll is using.
pub fn language() -> Language {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Language::English,
        _ => Language::German,
    }
}

pub fn set_language(language: Language) {
    let value = match language {
        Language::German => 0,
        Language::English => 1,
    };
    CURRENT.store(value, Ordering::Relaxed);
}

/// The language of the system, for users who never chose one.
pub fn system_language() -> Language {
    for variable in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(value) = std::env::var(variable)
            && !value.is_empty()
            && value != "C"
            && value != "POSIX"
        {
            return Language::of_locale(&value);
        }
    }
    Language::English
}

/// The German string in the language that is set.
pub fn translate(text: &'static str) -> &'static str {
    let catalog = catalog(language());
    match catalog.binary_search_by(|(id, _)| (*id).cmp(text)) {
        Ok(found) => catalog[found].1,
        Err(_) => text,
    }
}

/// Puts the arguments into the `{}` of a translated string. `{0}` picks an argument by its place,
/// for languages that need another order, and `{{` is a brace that stays one, as in Slint.
pub fn fill(template: &str, arguments: &[&dyn Display]) -> String {
    let mut result = String::with_capacity(template.len());
    let mut rest = template;
    let mut next = 0;
    while let Some(start) = rest.find(['{', '}']) {
        result.push_str(&rest[..start]);
        let brace = rest[start..].chars().next().unwrap_or('{');
        let after = &rest[start + 1..];
        if after.starts_with(brace) {
            result.push(brace);
            rest = &after[1..];
            continue;
        }
        let Some(end) = after.find('}').filter(|_| brace == '{') else {
            result.push(brace);
            rest = after;
            continue;
        };
        let inside = &after[..end];
        let argument = if inside.is_empty() {
            next += 1;
            arguments.get(next - 1)
        } else {
            inside
                .parse::<usize>()
                .ok()
                .and_then(|at| arguments.get(at))
        };
        match argument {
            Some(argument) => result.push_str(&argument.to_string()),
            None => {
                result.push('{');
                result.push_str(inside);
                result.push('}');
            }
        }
        rest = &after[end + 1..];
    }
    result.push_str(rest);
    result
}

/// A German string in the language that is set. Without arguments it stays a `&'static str`, with
/// arguments it becomes a `String` whose `{}` are filled in.
#[macro_export]
macro_rules! t {
    ($text:literal) => {
        $crate::translate($text)
    };
    ($text:literal, $($argument:expr),+ $(,)?) => {
        $crate::fill(
            $crate::translate($text),
            &[$(&$argument as &dyn ::std::fmt::Display),+],
        )
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_locale_names_the_language() {
        assert_eq!(Language::of_locale("de_DE.UTF-8"), Language::German);
        assert_eq!(Language::of_locale("de"), Language::German);
        assert_eq!(Language::of_locale("en_US.UTF-8"), Language::English);
        assert_eq!(
            Language::of_locale("fr_FR"),
            Language::English,
            "a language Atoll does not speak becomes English"
        );
    }

    #[test]
    fn languages_are_read_back_from_what_is_stored() {
        for language in Language::ALL {
            assert_eq!(Language::parse(language.tag()), Some(*language));
        }
        assert_eq!(Language::parse("kl"), None);
        assert_eq!(Language::German.bundle(), "", "German is the original");
    }

    #[test]
    fn arguments_go_where_the_braces_are() {
        assert_eq!(fill("{} von {}", &[&3, &7]), "3 von 7");
        assert_eq!(fill("{1}, {0}", &[&"eins", &"zwei"]), "zwei, eins");
        assert_eq!(
            fill("{} und {}", &[&1]),
            "1 und {}",
            "a missing argument leaves the braces alone"
        );
        assert_eq!(
            fill("nichts einzusetzen", &[]),
            "nichts einzusetzen",
            "text without braces is left as it is"
        );
        assert_eq!(
            fill("{{query}} wird zu {}", &[&"Atoll"]),
            "{query} wird zu Atoll",
            "a doubled brace stays a single one"
        );
    }

    #[test]
    fn a_string_without_a_translation_stays_german() {
        set_language(Language::English);
        assert_eq!(
            translate("Das gibt es nicht in der Datei"),
            "Das gibt es nicht in der Datei"
        );
        set_language(Language::German);
        assert_eq!(t!("Neuer Tab"), "Neuer Tab");
    }
}
