use atoll_core::{BookmarkId, IslandColor, Timestamp};
use atoll_storage::prelude::*;
use atoll_storage::{NewBookmark, NewIsland, NewTab, Storage};

fn storage() -> Storage {
    Storage::open_in_memory().expect("in-memory database")
}

fn link<'a>(parent: Option<BookmarkId>, title: &'a str, url: &'a str) -> NewBookmark<'a> {
    NewBookmark {
        parent_id: parent,
        title,
        url: Some(url),
        index: None,
        created_at: Timestamp(1),
    }
}

fn folder(parent: Option<BookmarkId>, title: &str) -> NewBookmark<'_> {
    NewBookmark {
        parent_id: parent,
        title,
        url: None,
        index: None,
        created_at: Timestamp(1),
    }
}

fn titles(storage: &Storage, parent: Option<BookmarkId>) -> Vec<String> {
    storage
        .bookmark_children(parent)
        .unwrap()
        .into_iter()
        .map(|bookmark| bookmark.title)
        .collect()
}

#[test]
fn bookmarks_live_in_folders_in_order() {
    let storage = storage();
    let rust = storage.create_bookmark(folder(None, "Rust")).unwrap();
    storage
        .create_bookmark(link(None, "Wikipedia", "https://wikipedia.org/"))
        .unwrap();
    let book = storage
        .create_bookmark(link(
            Some(rust.id),
            "Book",
            "https://doc.rust-lang.org/book/",
        ))
        .unwrap();
    storage
        .create_bookmark(NewBookmark {
            index: Some(0),
            ..link(Some(rust.id), "Std", "https://doc.rust-lang.org/std/")
        })
        .unwrap();

    assert!(rust.is_folder());
    assert!(!book.is_folder());
    assert_eq!(titles(&storage, None), ["Rust", "Wikipedia"]);
    assert_eq!(titles(&storage, Some(rust.id)), ["Std", "Book"]);
    assert_eq!(storage.bookmarks().unwrap().len(), 4);
    assert_eq!(
        storage
            .bookmark_by_url("https://doc.rust-lang.org/book/")
            .unwrap()
            .map(|b| b.id),
        Some(book.id)
    );
}

#[test]
fn moving_and_deleting_keeps_positions_dense() {
    let storage = storage();
    let archive = storage.create_bookmark(folder(None, "Archiv")).unwrap();
    let a = storage
        .create_bookmark(link(None, "A", "https://a.example/"))
        .unwrap();
    storage
        .create_bookmark(link(None, "B", "https://b.example/"))
        .unwrap();

    storage.move_bookmark(a.id, Some(archive.id), 0).unwrap();
    assert_eq!(titles(&storage, None), ["Archiv", "B"]);
    assert_eq!(titles(&storage, Some(archive.id)), ["A"]);

    storage
        .update_bookmark(a.id, "A2", Some("https://a2.example/"))
        .unwrap();
    assert_eq!(
        storage.bookmark(a.id).unwrap().url.as_deref(),
        Some("https://a2.example/")
    );

    storage.delete_bookmark(archive.id).unwrap();
    assert!(storage.bookmark(a.id).is_err());
    let positions: Vec<_> = storage
        .bookmark_children(None)
        .unwrap()
        .iter()
        .map(|b| b.position)
        .collect();
    assert_eq!(positions, [0]);
}

#[test]
fn folders_cannot_move_into_their_own_children() {
    let storage = storage();
    let outer = storage.create_bookmark(folder(None, "Außen")).unwrap();
    let inner = storage
        .create_bookmark(folder(Some(outer.id), "Innen"))
        .unwrap();
    assert!(storage.move_bookmark(outer.id, Some(inner.id), 0).is_err());
    assert!(storage.move_bookmark(outer.id, Some(outer.id), 0).is_err());
    assert_eq!(storage.bookmark(outer.id).unwrap().parent_id, None);
}

#[test]
fn visits_feed_the_global_history() {
    let storage = storage();
    let workspace = storage.create_workspace("Arbeit").unwrap();
    let island = storage
        .create_island(NewIsland {
            workspace_id: workspace.id,
            name: "A",
            color: IslandColor::Blue,
            index: None,
            created_at: Timestamp(0),
        })
        .unwrap();
    let tab = storage
        .create_tab(NewTab {
            private: false,
            island_id: island.id,
            url: "https://a.example/",
            title: None,
            index: None,
            created_at: Timestamp(1),
        })
        .unwrap();

    storage
        .record_visit(tab.id, "https://a.example/", None, Timestamp(1))
        .unwrap();
    storage
        .record_visit(tab.id, "https://b.example/", None, Timestamp(2))
        .unwrap();
    storage
        .record_visit(tab.id, "https://a.example/", None, Timestamp(3))
        .unwrap();
    storage
        .update_visit_title(tab.id, "https://a.example/", "Seite A")
        .unwrap();

    let pages = storage.visited_pages(10).unwrap();
    assert_eq!(pages[0].url, "https://a.example/");
    assert_eq!(pages[0].visit_count, 2);
    assert_eq!(pages[0].title.as_deref(), Some("Seite A"));

    let matches = storage.visited_pages_containing("seite", 10).unwrap();
    assert_eq!(matches.len(), 1);
    assert!(
        storage
            .visited_pages_containing("100%", 10)
            .unwrap()
            .is_empty()
    );

    storage.clear_history().unwrap();
    assert!(storage.visited_pages(10).unwrap().is_empty());
}

#[test]
fn tab_entries_span_all_workspaces_including_archived_islands() {
    let storage = storage();
    for (name, archived) in [("Arbeit", false), ("Privat", true)] {
        let workspace = storage.create_workspace(name).unwrap();
        let island = storage
            .create_island(NewIsland {
                workspace_id: workspace.id,
                name: "Insel",
                color: IslandColor::Blue,
                index: None,
                created_at: Timestamp(0),
            })
            .unwrap();
        storage
            .create_tab(NewTab {
                private: false,
                island_id: island.id,
                url: "https://example.com/",
                title: Some(name),
                index: None,
                created_at: Timestamp(1),
            })
            .unwrap();
        if archived {
            storage.archive_island(island.id, Timestamp(5)).unwrap();
        }
    }
    let entries = storage.all_tab_entries().unwrap();
    assert_eq!(entries.len(), 2);
    let archived = entries
        .iter()
        .find(|entry| entry.workspace_name == "Privat")
        .unwrap();
    assert!(archived.island.is_archived());
    assert_eq!(archived.tab.title.as_deref(), Some("Privat"));
}

#[test]
fn migration_backfills_history_from_tab_history() {
    let directory =
        std::env::temp_dir().join(format!("atoll-migration-test-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("atoll.db");
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(include_str!("../src/schema/v1_initial.sql"))
            .unwrap();
        connection
            .execute_batch(
                "PRAGMA user_version = 1;
                 INSERT INTO workspace (id, name, position) VALUES (1, 'W', 0);
                 INSERT INTO island (id, workspace_id, name, position) VALUES (1, 1, 'I', 0);
                 INSERT INTO tab (id, island_id, url, position, created_at) VALUES (1, 1, 'https://a.example/', 0, 0);
                 INSERT INTO tab_history (tab_id, url, title, visited_at) VALUES (1, 'https://a.example/', 'A', 5), (1, 'https://a.example/', 'A', 9);",
            )
            .unwrap();
    }
    let storage = Storage::open(&path).unwrap();
    let pages = storage.visited_pages(10).unwrap();
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].visit_count, 2);
    assert_eq!(pages[0].last_visited_at, Timestamp(9));
    drop(storage);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn the_history_is_read_in_pages_and_single_pages_can_be_forgotten() {
    let storage = storage();
    let workspace = storage.create_workspace("Arbeit").unwrap();
    let island = storage
        .create_island(NewIsland {
            workspace_id: workspace.id,
            name: "A",
            color: IslandColor::Blue,
            index: None,
            created_at: Timestamp(0),
        })
        .unwrap();
    let tab = storage
        .create_tab(NewTab {
            private: false,
            island_id: island.id,
            url: "https://a.example/",
            title: None,
            index: None,
            created_at: Timestamp(1),
        })
        .unwrap();
    for number in 1..=5 {
        let url = format!("https://page{number}.example/");
        storage
            .record_visit(tab.id, &url, Some("Seite"), Timestamp(number))
            .unwrap();
    }

    let first = storage.history_before(None, 2).unwrap();
    assert_eq!(first.len(), 2);
    assert_eq!(first[0].url, "https://page5.example/");
    let second = storage
        .history_before(Some(first[1].last_visited_at), 2)
        .unwrap();
    assert_eq!(second[0].url, "https://page3.example/");
    assert_eq!(
        second[1].url, "https://page2.example/",
        "the page after the last one continues where it stopped"
    );

    let found = storage
        .history_before_containing("page4", None, 10)
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].url, "https://page4.example/");

    storage.forget_page("https://page4.example/").unwrap();
    assert!(
        storage
            .history_before_containing("page4", None, 10)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        storage.tab_history(tab.id, 10).unwrap().len(),
        4,
        "the log of the tab forgets it too"
    );
}
