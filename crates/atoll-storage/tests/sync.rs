use atoll_core::{IslandColor, IslandId, TabId, Timestamp, WorkspaceId};
use atoll_storage::prelude::*;
use atoll_storage::{ArchiveReason, NewIsland, NewTab, Storage};

fn storage() -> Storage {
    Storage::open_in_memory().expect("in-memory database")
}

fn island(storage: &Storage, workspace: WorkspaceId, name: &str) -> IslandId {
    storage
        .create_island(NewIsland {
            workspace_id: workspace,
            name,
            color: IslandColor::Blue,
            index: None,
            created_at: Timestamp(0),
        })
        .unwrap()
        .id
}

fn tab(storage: &Storage, island: IslandId, url: &str, private: bool) -> TabId {
    storage
        .create_tab(NewTab {
            private,
            island_id: island,
            url,
            title: None,
            index: None,
            created_at: Timestamp(1),
        })
        .unwrap()
        .id
}

fn sync_id(storage: &Storage, table: &str, id: i64) -> String {
    storage
        .connection()
        .query_row(
            &format!("SELECT sync_id FROM {table} WHERE id = ?1"),
            [id],
            |row| row.get(0),
        )
        .unwrap()
}

/// The outbox as (kind, sync id, removed), and emptied afterwards.
fn take(storage: &Storage) -> Vec<(String, String, bool)> {
    let entries = storage.sync_outbox().unwrap();
    storage.forget_outbox(&entries).unwrap();
    entries
        .into_iter()
        .map(|entry| (entry.kind, entry.sync_id, entry.removed))
        .collect()
}

fn kinds(entries: &[(String, String, bool)]) -> Vec<(&str, bool)> {
    let mut kinds: Vec<(&str, bool)> = entries
        .iter()
        .map(|(kind, _, removed)| (kind.as_str(), *removed))
        .collect();
    kinds.sort();
    kinds
}

#[test]
fn every_new_record_gets_a_sync_id_and_is_written_down() {
    let storage = storage();
    let work = storage.create_workspace("Arbeit").unwrap();
    let docs = island(&storage, work.id, "Doku");
    let page = tab(&storage, docs, "https://a.example/", false);

    let entries = take(&storage);
    assert_eq!(
        kinds(&entries),
        [("island", false), ("tab", false), ("workspace", false)]
    );
    let id = sync_id(&storage, "tab", page.0);
    assert_eq!(id.len(), 36);
    assert_eq!(&id[14..15], "4");
    assert!(entries.iter().any(|(_, sync, _)| *sync == id));
    assert_ne!(sync_id(&storage, "island", docs.0), id);
}

#[test]
fn private_tabs_never_reach_the_outbox() {
    let storage = storage();
    let work = storage.create_workspace("Arbeit").unwrap();
    let docs = island(&storage, work.id, "Doku");
    take(&storage);

    let secret = tab(&storage, docs, "https://secret.example/", true);
    storage
        .update_tab_address(secret, "https://other-secret.example/")
        .unwrap();
    storage.delete_tab(secret).unwrap();
    assert!(take(&storage).is_empty());
}

#[test]
fn only_real_changes_of_synced_fields_are_written_down() {
    let storage = storage();
    let work = storage.create_workspace("Arbeit").unwrap();
    let docs = island(&storage, work.id, "Doku");
    let first = tab(&storage, docs, "https://a.example/", false);
    tab(&storage, docs, "https://b.example/", false);
    take(&storage);

    storage.set_tab_scroll(first, Some(400)).unwrap();
    assert!(
        take(&storage).is_empty(),
        "the scroll position stays on the device"
    );

    storage.move_tab(first, docs, 0).unwrap();
    assert!(
        take(&storage).is_empty(),
        "renumbering to the same places changes nothing"
    );

    storage.move_tab(first, docs, 1).unwrap();
    assert_eq!(kinds(&take(&storage)), [("tab", false), ("tab", false)]);

    storage.update_tab_title(first, Some("A")).unwrap();
    storage.update_tab_title(first, Some("AA")).unwrap();
    assert_eq!(
        take(&storage).len(),
        1,
        "two changes of one tab leave one entry"
    );
}

#[test]
fn removals_are_written_down_also_when_they_cascade() {
    let storage = storage();
    let work = storage.create_workspace("Arbeit").unwrap();
    let docs = island(&storage, work.id, "Doku");
    let page = tab(&storage, docs, "https://a.example/", false);
    let page_id = sync_id(&storage, "tab", page.0);
    take(&storage);

    storage.delete_island(docs).unwrap();
    let entries = take(&storage);
    assert_eq!(kinds(&entries), [("island", true), ("tab", true)]);
    assert!(
        entries
            .iter()
            .any(|(_, id, removed)| *id == page_id && *removed)
    );
}

#[test]
fn archiving_writes_down_both_sides() {
    let storage = storage();
    let work = storage.create_workspace("Arbeit").unwrap();
    let docs = island(&storage, work.id, "Doku");
    let page = tab(&storage, docs, "https://a.example/", false);
    take(&storage);

    storage
        .archive_tabs(&[page], Timestamp(5), ArchiveReason::Asked)
        .unwrap();
    assert_eq!(
        kinds(&take(&storage)),
        [
            ("archived_island", false),
            ("archived_tab", false),
            ("archived_workspace", false),
            ("tab", true)
        ]
    );
}

#[test]
fn nothing_is_written_down_while_changes_are_taken_over() {
    let storage = storage();
    let work = storage.create_workspace("Arbeit").unwrap();
    take(&storage);

    storage.set_sync_applying(true).unwrap();
    let docs = island(&storage, work.id, "Vom Server");
    tab(&storage, docs, "https://a.example/", false);
    storage.rename_workspace(work.id, "Anders").unwrap();
    storage.set_sync_applying(false).unwrap();
    assert!(take(&storage).is_empty());
    assert_eq!(
        sync_id(&storage, "island", docs.0).len(),
        36,
        "ids are still given"
    );

    storage.rename_workspace(work.id, "Wieder").unwrap();
    assert_eq!(kinds(&take(&storage)), [("workspace", false)]);
}

#[test]
fn a_record_given_its_sync_id_keeps_it() {
    let storage = storage();
    let work = storage.create_workspace("Arbeit").unwrap();
    let docs = island(&storage, work.id, "Doku");
    storage
        .connection()
        .execute(
            "INSERT INTO tab (island_id, url, position, created_at, sync_id)
             VALUES (?1, 'https://a.example/', 0, 1, '01890a5d-ac96-4b4e-8f3c-7a1b2c3d4e5f')",
            [docs.0],
        )
        .unwrap();
    let id: String = storage
        .connection()
        .query_row(
            "SELECT sync_id FROM tab WHERE url = 'https://a.example/'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(id, "01890a5d-ac96-4b4e-8f3c-7a1b2c3d4e5f");
}

#[test]
fn an_entry_changed_while_it_was_sent_stays() {
    let storage = storage();
    storage.create_workspace("Arbeit").unwrap();
    let read = storage.sync_outbox().unwrap();
    storage
        .connection()
        .execute("UPDATE sync_outbox SET changed_at = changed_at + 1", [])
        .unwrap();
    storage.forget_outbox(&read).unwrap();
    assert_eq!(storage.sync_outbox().unwrap().len(), 1);
}

#[test]
fn records_from_before_get_sync_ids_of_their_own() {
    let directory =
        std::env::temp_dir().join(format!("atoll-sync-migration-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("atoll.db");
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        for migration in [
            include_str!("../src/schema/v1_initial.sql"),
            include_str!("../src/schema/v2_bookmarks_and_history.sql"),
            include_str!("../src/schema/v3_downloads.sql"),
            include_str!("../src/schema/v4_windows.sql"),
            include_str!("../src/schema/v5_adblock.sql"),
            include_str!("../src/schema/v6_island_usage.sql"),
            include_str!("../src/schema/v7_download_kind.sql"),
            include_str!("../src/schema/v8_plugins.sql"),
            include_str!("../src/schema/v9_private_tabs.sql"),
            include_str!("../src/schema/v10_adblock_rules.sql"),
            include_str!("../src/schema/v11_vpn_rules.sql"),
            include_str!("../src/schema/v12_page_text.sql"),
            include_str!("../src/schema/v13_tab_archive.sql"),
            include_str!("../src/schema/v14_archive_reason.sql"),
        ] {
            connection.execute_batch(migration).unwrap();
        }
        connection
            .execute_batch(
                "PRAGMA user_version = 14;
                 INSERT INTO workspace (id, name, position) VALUES (1, 'Arbeit', 0);
                 INSERT INTO island (id, workspace_id, name, position) VALUES (1, 1, 'A', 0), (2, 1, 'B', 1);
                 INSERT INTO tab (island_id, url, position, created_at) VALUES (1, 'https://a.example/', 0, 0), (1, 'https://b.example/', 1, 0), (2, 'https://c.example/', 0, 0);",
            )
            .unwrap();
    }

    let storage = Storage::open(&path).unwrap();
    let ids: Vec<String> = storage
        .connection()
        .prepare("SELECT sync_id FROM tab UNION ALL SELECT sync_id FROM island UNION ALL SELECT sync_id FROM workspace")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(ids.len(), 6);
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), 6, "every record has an id of its own");
    assert!(ids.iter().all(|id| id.len() == 36));
    assert!(
        storage.sync_outbox().unwrap().is_empty(),
        "what was there before is sent when sync is set up, not as changes"
    );
    drop(storage);
    std::fs::remove_dir_all(directory).unwrap();
}
