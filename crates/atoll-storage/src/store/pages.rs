//! What the open pages say, so they can be searched.
//!
//! Only the text a page shows is kept, never its markup: one row per open tab, replaced when the
//! page is left and thrown away with the tab. SQLite's full text index does the searching, so
//! nothing of this has to be held in memory, and a search costs the matches, not the pages.

use atoll_core::{TabId, Timestamp};
use rusqlite::params;

use crate::connection::Connected;
use crate::error::StorageResult;

/// How much text of one page is kept. A long article is a few thousand characters; this is
/// generous and still bounds what the biggest page can cost.
pub const MAX_PAGE_TEXT: usize = 64 * 1024;

/// A page the search found, with the piece of text the words were found in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageHit {
    pub tab: TabId,
    pub url: String,
    pub title: String,
    /// The words in their sentence, with the match between the marks, empty when only the
    /// address and the title were searched.
    pub snippet: String,
}

/// The text of the open pages.
pub trait PageTextStore: Connected {
    /// Remembers what one page shows. An earlier text of the same tab is replaced.
    fn set_page_text(
        &self,
        tab: TabId,
        url: &str,
        title: &str,
        text: &str,
        _at: Timestamp,
    ) -> StorageResult<()> {
        let text = shortened(text);
        self.connection()
            .prepare_cached("DELETE FROM page_text WHERE rowid = ?1")?
            .execute([tab.0])?;
        self.connection()
            .prepare_cached(
                "INSERT INTO page_text (rowid, url, title, text) VALUES (?1, ?2, ?3, ?4)",
            )?
            .execute(params![tab.0, url, title, text])?;
        Ok(())
    }

    /// How much is remembered, for the settings: how many pages and how much text.
    fn page_text_size(&self) -> StorageResult<(usize, u64)> {
        let mut statement = self
            .connection()
            .prepare_cached("SELECT count(*), coalesce(sum(length(text)), 0) FROM page_text")?;
        let size = statement.query_row([], |row| {
            Ok((row.get::<_, i64>(0)? as usize, row.get::<_, i64>(1)? as u64))
        })?;
        Ok(size)
    }

    /// Throws away everything that was remembered of the pages.
    fn forget_all_page_texts(&self) -> StorageResult<()> {
        self.connection()
            .prepare_cached("DELETE FROM page_text")?
            .execute([])?;
        Ok(())
    }

    fn forget_page_text(&self, tab: TabId) -> StorageResult<()> {
        self.connection()
            .prepare_cached("DELETE FROM page_text WHERE rowid = ?1")?
            .execute([tab.0])?;
        Ok(())
    }

    /// Throws away what belongs to tabs that are gone, which is how a crash is cleaned up after.
    fn forget_closed_page_texts(&self) -> StorageResult<usize> {
        let gone = self
            .connection()
            .prepare_cached("DELETE FROM page_text WHERE rowid NOT IN (SELECT id FROM tab)")?
            .execute([])?;
        Ok(gone)
    }

    /// Looks for `words` in the text of the open pages, the best matches first.
    fn search_page_text(&self, words: &str, limit: usize) -> StorageResult<Vec<PageHit>> {
        let Some(query) = fts_query(words) else {
            return Ok(Vec::new());
        };
        let mut statement = self.connection().prepare_cached(
            "SELECT rowid, url, title, snippet(page_text, 2, '', '', '…', 12)
             FROM page_text WHERE page_text MATCH ?1 ORDER BY rank LIMIT ?2",
        )?;
        let hits = statement
            .query_map(params![query, limit as i64], |row| {
                Ok(PageHit {
                    tab: TabId(row.get(0)?),
                    url: row.get(1)?,
                    title: row.get(2)?,
                    snippet: row.get(3)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(hits)
    }
}

impl<T: Connected + ?Sized> PageTextStore for T {}

/// Keeps the text within bounds, at a character boundary.
fn shortened(text: &str) -> &str {
    if text.len() <= MAX_PAGE_TEXT {
        return text;
    }
    let mut end = MAX_PAGE_TEXT;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// What the user typed, as the full text index reads it: every word on its own, each one allowed
/// to be the beginning of a longer one, and nothing of what they typed taken as an operator.
fn fts_query(words: &str) -> Option<String> {
    let query: Vec<String> = words
        .split_whitespace()
        .map(|word| word.replace('"', ""))
        .filter(|word| !word.is_empty())
        .map(|word| format!("\"{word}\"*"))
        .collect();
    (!query.is_empty()).then(|| query.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Storage;

    fn storage() -> Storage {
        Storage::open_in_memory().expect("in-memory database")
    }

    #[test]
    fn the_text_of_a_page_is_found_by_its_words() {
        let storage = storage();
        storage
            .set_page_text(
                TabId(1),
                "https://example.org/rezept",
                "Brot backen",
                "Für den Teig brauchst du Mehl, Wasser und Salz. Danach ruht er zwölf Stunden.",
                Timestamp(1),
            )
            .unwrap();
        storage
            .set_page_text(
                TabId(2),
                "https://example.org/wetter",
                "Wetter",
                "Morgen regnet es den ganzen Tag.",
                Timestamp(1),
            )
            .unwrap();

        let hits = storage.search_page_text("mehl", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].tab, TabId(1));
        assert_eq!(hits[0].title, "Brot backen");
        assert!(
            hits[0].snippet.contains("Mehl"),
            "the words come back in their sentence: {}",
            hits[0].snippet
        );

        assert!(
            !storage.search_page_text("reg", 10).unwrap().is_empty(),
            "the beginning of a word is enough"
        );
        assert!(storage.search_page_text("sonne", 10).unwrap().is_empty());
    }

    #[test]
    fn every_word_has_to_be_there() {
        let storage = storage();
        storage
            .set_page_text(
                TabId(1),
                "https://example.org/",
                "Seite",
                "Mehl und Wasser",
                Timestamp(1),
            )
            .unwrap();
        assert_eq!(
            storage.search_page_text("mehl wasser", 10).unwrap().len(),
            1
        );
        assert!(
            storage
                .search_page_text("mehl hefe", 10)
                .unwrap()
                .is_empty(),
            "what is not on the page does not match it"
        );
    }

    #[test]
    fn a_page_that_was_left_replaces_what_it_said_before() {
        let storage = storage();
        for text in ["das erste", "das zweite"] {
            storage
                .set_page_text(
                    TabId(1),
                    "https://example.org/",
                    "Seite",
                    text,
                    Timestamp(1),
                )
                .unwrap();
        }
        assert!(storage.search_page_text("erste", 10).unwrap().is_empty());
        assert_eq!(storage.search_page_text("zweite", 10).unwrap().len(), 1);
    }

    #[test]
    fn what_belongs_to_a_closed_tab_goes_away() {
        let storage = storage();
        storage
            .set_page_text(
                TabId(7),
                "https://example.org/",
                "Seite",
                "etwas",
                Timestamp(1),
            )
            .unwrap();
        storage.forget_page_text(TabId(7)).unwrap();
        assert!(storage.search_page_text("etwas", 10).unwrap().is_empty());

        storage
            .set_page_text(
                TabId(8),
                "https://example.org/",
                "Seite",
                "etwas",
                Timestamp(1),
            )
            .unwrap();
        let gone = storage.forget_closed_page_texts().unwrap();
        assert_eq!(gone, 1, "no tab of that number exists any more");
        assert!(storage.search_page_text("etwas", 10).unwrap().is_empty());
    }

    #[test]
    fn what_is_remembered_can_be_counted_and_thrown_away() {
        let storage = storage();
        assert_eq!(storage.page_text_size().unwrap(), (0, 0));
        for tab in [1, 2] {
            storage
                .set_page_text(
                    TabId(tab),
                    "https://example.org/",
                    "Seite",
                    "zwölf Zeichen",
                    Timestamp(1),
                )
                .unwrap();
        }
        let (pages, bytes) = storage.page_text_size().unwrap();
        assert_eq!(pages, 2);
        assert!(bytes > 0);

        storage.forget_all_page_texts().unwrap();
        assert_eq!(storage.page_text_size().unwrap(), (0, 0));
        assert!(storage.search_page_text("zwölf", 10).unwrap().is_empty());
    }

    #[test]
    fn what_the_user_types_is_never_read_as_an_instruction() {
        assert_eq!(fts_query("mehl"), Some("\"mehl\"*".to_owned()));
        assert_eq!(
            fts_query("  mehl   wasser "),
            Some("\"mehl\"* \"wasser\"*".to_owned())
        );
        assert_eq!(
            fts_query("OR NEAR(\"x\")"),
            Some("\"OR\"* \"NEAR(x)\"*".to_owned()),
            "an operator of the index is only a word here"
        );
        assert_eq!(fts_query("   "), None);
    }

    #[test]
    fn a_page_that_says_too_much_is_cut_at_a_character() {
        let long = "ä".repeat(MAX_PAGE_TEXT);
        let kept = shortened(&long);
        assert!(kept.len() <= MAX_PAGE_TEXT);
        assert!(kept.chars().all(|character| character == 'ä'));
    }
}
