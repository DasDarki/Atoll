//! Reading Opera's history, which it keeps in the SQLite file Chromium writes.
//!
//! Opera may be running while this happens, so the file is copied away first and only the copy
//! is opened. Chromium counts microseconds since 1601, Atoll counts milliseconds since 1970.

use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use crate::opera::OperaError;

/// Microseconds between 1601-01-01 and 1970-01-01.
const EPOCH_DIFFERENCE: i64 = 11_644_473_600_000_000;

/// How many pages are taken over at most, newest first.
const MAX_VISITS: usize = 50_000;

/// A page Opera remembers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperaVisit {
    pub url: String,
    pub title: Option<String>,
    pub visit_count: i64,
    /// Milliseconds since 1970, as the rest of Atoll counts.
    pub last_visited_at: i64,
}

/// Reads the history of an Opera profile, newest first.
pub fn read_history(directory: &Path) -> Result<Vec<OperaVisit>, OperaError> {
    let source = directory.join("History");
    if !source.is_file() {
        return Err(OperaError::Unreadable(
            "History: the file is missing".to_owned(),
        ));
    }
    let copy = Copy::of(&source)?;
    let connection = Connection::open_with_flags(
        copy.path(),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| OperaError::Unreadable(format!("History: {error}")))?;
    let mut statement = connection
        .prepare(
            "SELECT url, title, visit_count, last_visit_time FROM urls
             WHERE hidden = 0 AND last_visit_time > 0
             ORDER BY last_visit_time DESC LIMIT ?1",
        )
        .map_err(|error| OperaError::Unreadable(format!("History: {error}")))?;
    let visits = statement
        .query_map([MAX_VISITS as i64], |row| {
            Ok(OperaVisit {
                url: row.get(0)?,
                title: row
                    .get::<_, Option<String>>(1)?
                    .filter(|title| !title.trim().is_empty()),
                visit_count: row.get::<_, i64>(2)?.max(1),
                last_visited_at: unix_millis(row.get(3)?),
            })
        })
        .map_err(|error| OperaError::Unreadable(format!("History: {error}")))?
        .filter_map(Result::ok)
        .filter(|visit| visit.url.starts_with("http://") || visit.url.starts_with("https://"))
        .collect();
    Ok(visits)
}

/// The moment Chromium wrote, in the milliseconds Atoll counts.
fn unix_millis(chromium_microseconds: i64) -> i64 {
    (chromium_microseconds - EPOCH_DIFFERENCE) / 1_000
}

/// A copy of a file that goes away again, so that a running Opera is left alone. It is created
/// fresh and readable by its owner only, because it holds the history of the user.
struct Copy(std::path::PathBuf);

impl Copy {
    fn of(source: &Path) -> Result<Self, OperaError> {
        let unreadable =
            |error: std::io::Error| OperaError::Unreadable(format!("History: {error}"));
        let name = format!(
            "atoll-opera-history-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        );
        let path = std::env::temp_dir().join(name);
        let _ = std::fs::remove_file(&path);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut target = options.open(&path).map_err(unreadable)?;
        let mut original = std::fs::File::open(source).map_err(unreadable)?;
        std::io::copy(&mut original, &mut target).map_err(unreadable)?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Copy {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_times_of_chromium_become_the_times_of_atoll() {
        assert_eq!(unix_millis(EPOCH_DIFFERENCE), 0);
        assert_eq!(unix_millis(EPOCH_DIFFERENCE + 1_500_000), 1_500);
    }

    #[test]
    fn pages_come_back_newest_first_and_without_the_hidden_ones() {
        let directory = std::env::temp_dir().join(format!(
            "atoll-opera-history-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let connection = Connection::open(directory.join("History")).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE urls (id INTEGER PRIMARY KEY, url TEXT, title TEXT,
                     visit_count INTEGER, typed_count INTEGER, last_visit_time INTEGER,
                     hidden INTEGER);
                 INSERT INTO urls VALUES
                     (1, 'https://a.example/', 'A', 3, 0, 13300000000000000, 0),
                     (2, 'https://b.example/', '', 1, 0, 13400000000000000, 0),
                     (3, 'https://c.example/', 'C', 1, 0, 13500000000000000, 1),
                     (4, 'opera://startpage', 'Start', 9, 0, 13500000000000000, 0);",
            )
            .unwrap();
        drop(connection);

        let visits = read_history(&directory).unwrap();
        assert_eq!(visits.len(), 2, "hidden pages and Opera's own stay behind");
        assert_eq!(visits[0].url, "https://b.example/");
        assert_eq!(visits[0].title, None, "an empty title counts as none");
        assert_eq!(visits[1].url, "https://a.example/");
        assert_eq!(visits[1].visit_count, 3);
        assert_eq!(
            visits[1].last_visited_at,
            unix_millis(13_300_000_000_000_000)
        );
        let _ = std::fs::remove_dir_all(&directory);
    }
}
