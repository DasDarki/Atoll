use atoll_core::{IslandColor, IslandId, TabId, Timestamp, WorkspaceId};
use atoll_storage::prelude::*;
use atoll_storage::{NewIsland, NewTab, Storage};

fn storage() -> Storage {
    Storage::open_in_memory().expect("in-memory database")
}

fn island(storage: &Storage, workspace: WorkspaceId, name: &str, color: IslandColor) -> IslandId {
    storage
        .create_island(NewIsland {
            workspace_id: workspace,
            name,
            color,
            index: None,
            created_at: Timestamp(0),
        })
        .unwrap()
        .id
}

fn tab(storage: &Storage, island: IslandId, url: &str) -> TabId {
    storage
        .create_tab(NewTab {
            private: false,
            island_id: island,
            url,
            title: None,
            index: None,
            created_at: Timestamp(1),
        })
        .unwrap()
        .id
}

fn urls(storage: &Storage, island: IslandId) -> Vec<String> {
    storage
        .tabs_in_island(island)
        .unwrap()
        .into_iter()
        .map(|tab| tab.url)
        .collect()
}

#[test]
fn an_archived_tab_keeps_its_island_and_workspace_and_comes_back_into_them() {
    let storage = storage();
    let work = storage.create_workspace("Arbeit").unwrap();
    let docs = island(&storage, work.id, "Doku", IslandColor::Teal);
    tab(&storage, docs, "https://a.example/");
    let b = tab(&storage, docs, "https://b.example/");
    tab(&storage, docs, "https://c.example/");

    storage.archive_tabs(&[b], Timestamp(50)).unwrap();
    assert_eq!(
        urls(&storage, docs),
        ["https://a.example/", "https://c.example/"]
    );

    let archive = storage.archive().unwrap();
    assert_eq!(archive.len(), 1);
    assert_eq!(archive[0].name, "Arbeit");
    assert_eq!(archive[0].workspace_id, Some(work.id));
    let archived = &archive[0].islands[0];
    assert_eq!(archived.island_id, Some(docs));
    assert_eq!(archived.name, "Doku");
    assert_eq!(archived.color, IslandColor::Teal);
    assert_eq!(archived.tabs.len(), 1);
    assert_eq!(archived.tabs[0].position, 1);
    assert_eq!(archived.tabs[0].archived_at, Timestamp(50));

    let back = storage
        .restore_archived_tab(archived.tabs[0].id, Timestamp(90))
        .unwrap();
    assert_eq!(
        urls(&storage, docs),
        [
            "https://a.example/",
            "https://b.example/",
            "https://c.example/"
        ],
        "back at the place it had"
    );
    assert_eq!(
        storage.tab(back).unwrap().last_used_at,
        Some(Timestamp(90)),
        "it counts as used, or it would go straight back"
    );
    assert!(
        storage.archive().unwrap().is_empty(),
        "nothing is left behind"
    );
}

#[test]
fn a_tab_whose_island_is_gone_comes_back_into_one_made_alike() {
    let storage = storage();
    let work = storage.create_workspace("Arbeit").unwrap();
    island(&storage, work.id, "Vorne", IslandColor::Blue);
    let reading = island(&storage, work.id, "Lesen", IslandColor::Rose);
    island(&storage, work.id, "Hinten", IslandColor::Blue);
    let first = tab(&storage, reading, "https://one.example/");
    let second = tab(&storage, reading, "https://two.example/");

    storage
        .archive_tabs(&[first, second], Timestamp(5))
        .unwrap();
    storage.delete_island(reading).unwrap();
    let archived = storage.archive().unwrap()[0].islands[0].clone();
    assert_eq!(archived.island_id, None, "the link goes with the island");

    storage
        .restore_archived_tab(archived.tabs[1].id, Timestamp(9))
        .unwrap();
    let islands = storage.islands(work.id).unwrap();
    let names: Vec<&str> = islands.iter().map(|island| island.name.as_str()).collect();
    assert_eq!(
        names,
        ["Vorne", "Lesen", "Hinten"],
        "made again at the place it had"
    );
    let made = islands[1].clone();
    assert_eq!(made.color, IslandColor::Rose);
    assert_ne!(made.id, reading);

    storage
        .restore_archived_tab(archived.tabs[0].id, Timestamp(9))
        .unwrap();
    assert_eq!(
        storage.islands(work.id).unwrap().len(),
        3,
        "the second tab follows into the island made for the first"
    );
    assert_eq!(
        urls(&storage, made.id),
        ["https://one.example/", "https://two.example/"]
    );
}

#[test]
fn an_id_handed_out_again_draws_no_tab_into_a_stranger() {
    let storage = storage();
    let work = storage.create_workspace("Arbeit").unwrap();
    let old = island(&storage, work.id, "Alt", IslandColor::Amber);
    let archived_tab = tab(&storage, old, "https://old.example/");
    storage.archive_tabs(&[archived_tab], Timestamp(5)).unwrap();
    storage.delete_island(old).unwrap();

    let stranger = island(&storage, work.id, "Fremd", IslandColor::Blue);
    assert_eq!(stranger, old, "SQLite hands the highest id out again");

    let id = storage.archive().unwrap()[0].islands[0].tabs[0].id;
    let back = storage.restore_archived_tab(id, Timestamp(9)).unwrap();
    let home = storage.tab(back).unwrap().island_id;
    assert_ne!(home, stranger);
    assert_eq!(storage.island(home).unwrap().name, "Alt");
}

#[test]
fn a_whole_workspace_comes_back_when_it_was_deleted() {
    let storage = storage();
    let first = storage.create_workspace("Eins").unwrap();
    let travel = storage.create_workspace("Reisen").unwrap();
    storage.create_workspace("Drei").unwrap();
    let flights = island(&storage, travel.id, "Flüge", IslandColor::Cyan);
    let hotels = island(&storage, travel.id, "Hotels", IslandColor::Green);
    tab(&storage, flights, "https://fly.example/");
    tab(&storage, hotels, "https://sleep.example/");
    tab(&storage, hotels, "https://stay.example/");

    storage.archive_island(flights, Timestamp(5)).unwrap();
    storage.archive_island(hotels, Timestamp(5)).unwrap();
    storage.delete_workspace(travel.id).unwrap();
    let archive = storage.archive().unwrap();
    assert_eq!(
        archive[0].workspace_id, None,
        "the link goes with the workspace"
    );
    assert_eq!(archive[0].islands.len(), 2);

    let restored = storage
        .restore_archived_workspace(archive[0].id, Timestamp(9))
        .unwrap();
    assert_eq!(restored.len(), 3);
    let workspaces = storage.workspaces().unwrap();
    let names: Vec<&str> = workspaces.iter().map(|w| w.name.as_str()).collect();
    assert_eq!(names, ["Eins", "Reisen", "Drei"]);
    let made = &workspaces[1];
    assert_ne!(made.id, first.id);
    let islands: Vec<String> = storage
        .islands(made.id)
        .unwrap()
        .into_iter()
        .map(|island| island.name)
        .collect();
    assert_eq!(islands, ["Flüge", "Hotels"]);
    assert!(storage.archive().unwrap().is_empty());
}

#[test]
fn archiving_an_island_keeps_the_order_of_its_tabs_and_removes_it() {
    let storage = storage();
    let work = storage.create_workspace("Arbeit").unwrap();
    let keep = island(&storage, work.id, "Bleibt", IslandColor::Blue);
    let gone = island(&storage, work.id, "Geht", IslandColor::Violet);
    tab(&storage, keep, "https://keep.example/");
    for url in [
        "https://1.example/",
        "https://2.example/",
        "https://3.example/",
    ] {
        tab(&storage, gone, url);
    }

    storage.archive_island(gone, Timestamp(5)).unwrap();
    assert_eq!(storage.islands(work.id).unwrap().len(), 1);
    let archived = storage.archive().unwrap()[0].islands[0].clone();
    let shown: Vec<&str> = archived.tabs.iter().map(|tab| tab.url.as_str()).collect();
    assert_eq!(
        shown,
        [
            "https://1.example/",
            "https://2.example/",
            "https://3.example/"
        ],
        "the archive shows them as the sidebar did"
    );

    storage
        .restore_archived_island(archived.id, Timestamp(9))
        .unwrap();
    let islands = storage.islands(work.id).unwrap();
    assert_eq!(islands[1].name, "Geht");
    assert_eq!(
        urls(&storage, islands[1].id),
        [
            "https://1.example/",
            "https://2.example/",
            "https://3.example/"
        ]
    );
}

#[test]
fn an_island_moved_elsewhere_takes_its_archived_tabs_along() {
    let storage = storage();
    let work = storage.create_workspace("Arbeit").unwrap();
    let home = storage.create_workspace("Zuhause").unwrap();
    let wander = island(&storage, work.id, "Wandert", IslandColor::Blue);
    let first = tab(&storage, wander, "https://first.example/");
    let second = tab(&storage, wander, "https://second.example/");

    storage.archive_tabs(&[first], Timestamp(5)).unwrap();
    storage.move_island(wander, home.id, 0).unwrap();
    storage.archive_tabs(&[second], Timestamp(6)).unwrap();

    let archive = storage.archive().unwrap();
    assert_eq!(
        archive.len(),
        1,
        "the copy of the old workspace is not needed any more"
    );
    assert_eq!(archive[0].name, "Zuhause");
    assert_eq!(archive[0].islands[0].tabs.len(), 2);
}

#[test]
fn history_goes_into_the_archive_and_comes_back() {
    let storage = storage();
    let work = storage.create_workspace("Arbeit").unwrap();
    let docs = island(&storage, work.id, "Doku", IslandColor::Blue);
    let page = tab(&storage, docs, "https://b.example/");
    storage
        .record_visit(page, "https://a.example/", Some("A"), Timestamp(2))
        .unwrap();
    storage
        .record_visit(page, "https://b.example/", Some("B"), Timestamp(3))
        .unwrap();

    storage.archive_tabs(&[page], Timestamp(5)).unwrap();
    let id = storage.archive().unwrap()[0].islands[0].tabs[0].id;
    let back = storage.restore_archived_tab(id, Timestamp(9)).unwrap();
    let history: Vec<String> = storage
        .tab_history(back, 10)
        .unwrap()
        .into_iter()
        .rev()
        .map(|entry| entry.url)
        .collect();
    assert_eq!(history, ["https://a.example/", "https://b.example/"]);
}

#[test]
fn tabs_that_left_one_after_another_behind_one_that_stayed_keep_their_order() {
    let storage = storage();
    let work = storage.create_workspace("Arbeit").unwrap();
    let docs = island(&storage, work.id, "Doku", IslandColor::Blue);
    tab(&storage, docs, "https://stays.example/");
    let a = tab(&storage, docs, "https://a.example/");
    let b = tab(&storage, docs, "https://b.example/");
    tab(&storage, docs, "https://also-stays.example/");

    storage.archive_tabs(&[a, b], Timestamp(5)).unwrap();
    let shown: Vec<String> = storage.archive().unwrap()[0].islands[0]
        .tabs
        .iter()
        .map(|tab| tab.url.clone())
        .collect();
    assert_eq!(shown, ["https://a.example/", "https://b.example/"]);

    let archived = storage.archive().unwrap()[0].islands[0].id;
    storage
        .restore_archived_island(archived, Timestamp(9))
        .unwrap();
    assert_eq!(
        urls(&storage, docs),
        [
            "https://stays.example/",
            "https://a.example/",
            "https://b.example/",
            "https://also-stays.example/"
        ]
    );
}

#[test]
fn only_tabs_unused_for_long_enough_are_due_and_pinned_or_private_ones_never() {
    let storage = storage();
    let work = storage.create_workspace("Arbeit").unwrap();
    let docs = island(&storage, work.id, "Doku", IslandColor::Blue);
    let old = tab(&storage, docs, "https://old.example/");
    let fresh = tab(&storage, docs, "https://fresh.example/");
    let pinned = tab(&storage, docs, "https://pinned.example/");
    let never = tab(&storage, docs, "https://never.example/");
    storage
        .create_tab(NewTab {
            private: true,
            island_id: docs,
            url: "https://private.example/",
            title: None,
            index: None,
            created_at: Timestamp(1),
        })
        .unwrap();
    storage.touch_tab(old, Timestamp(10)).unwrap();
    storage.touch_tab(fresh, Timestamp(500)).unwrap();
    storage.touch_tab(pinned, Timestamp(10)).unwrap();
    storage.set_tab_pinned(pinned, true).unwrap();

    let due: Vec<TabId> = storage
        .tabs_unused_since(Timestamp(100))
        .unwrap()
        .into_iter()
        .map(|tab| tab.id)
        .collect();
    assert_eq!(
        due,
        [old, never],
        "a tab never shown counts from when it was opened"
    );
}

#[test]
fn deleting_from_the_archive_takes_empty_copies_along() {
    let storage = storage();
    let work = storage.create_workspace("Arbeit").unwrap();
    let docs = island(&storage, work.id, "Doku", IslandColor::Blue);
    let a = tab(&storage, docs, "https://a.example/");
    let b = tab(&storage, docs, "https://b.example/");
    storage.archive_tabs(&[a, b], Timestamp(5)).unwrap();
    assert_eq!(storage.archived_tab_count().unwrap(), 2);

    let ids: Vec<i64> = storage.archive().unwrap()[0].islands[0]
        .tabs
        .iter()
        .map(|tab| tab.id)
        .collect();
    storage.delete_archived_tab(ids[0]).unwrap();
    assert_eq!(storage.archived_tab_count().unwrap(), 1);
    storage.delete_archived_tab(ids[1]).unwrap();
    assert!(storage.archive().unwrap().is_empty());
}

#[test]
fn the_island_archive_of_before_moves_into_the_new_one() {
    let directory =
        std::env::temp_dir().join(format!("atoll-archive-migration-{}", std::process::id()));
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
        ] {
            connection.execute_batch(migration).unwrap();
        }
        connection
            .execute_batch(
                "PRAGMA user_version = 12;
                 INSERT INTO workspace (id, name, position) VALUES (1, 'Arbeit', 0), (2, 'Privat', 1);
                 INSERT INTO island (id, workspace_id, name, position, color, archived_at)
                 VALUES (1, 1, 'Offen', 0, 1, NULL),
                        (2, 1, 'Alt', 1, 6, 700),
                        (3, 2, 'Urlaub', 4, 3, 800);
                 INSERT INTO tab (id, island_id, url, title, position, created_at, last_used_at)
                 VALUES (1, 1, 'https://open.example/', 'Offen', 0, 0, 10),
                        (2, 2, 'https://old-a.example/', 'A', 0, 0, 20),
                        (3, 2, 'https://old-b.example/', 'B', 1, 0, 30),
                        (4, 3, 'https://beach.example/', 'Strand', 0, 0, 40);
                 INSERT INTO tab_history (tab_id, url, title, visited_at)
                 VALUES (3, 'https://before.example/', 'Vorher', 25), (3, 'https://old-b.example/', 'B', 30);",
            )
            .unwrap();
    }

    let storage = Storage::open(&path).unwrap();
    assert_eq!(storage.islands(WorkspaceId(1)).unwrap().len(), 1);
    assert!(storage.islands(WorkspaceId(2)).unwrap().is_empty());
    assert_eq!(storage.tab(TabId(1)).unwrap().url, "https://open.example/");

    let archive = storage.archive().unwrap();
    let names: Vec<&str> = archive.iter().map(|w| w.name.as_str()).collect();
    assert_eq!(names, ["Arbeit", "Privat"]);
    let old = &archive[0].islands[0];
    assert_eq!(old.name, "Alt");
    assert_eq!(old.color, IslandColor::Rose);
    assert_eq!(
        old.island_id, None,
        "the island itself is gone, it is made again"
    );
    let titles: Vec<&str> = old.tabs.iter().map(|tab| tab.display_title()).collect();
    assert_eq!(titles, ["A", "B"]);
    assert_eq!(old.tabs[0].archived_at, Timestamp(700));
    assert_eq!(archive[1].islands[0].tabs[0].url, "https://beach.example/");

    let back = storage
        .restore_archived_tab(old.tabs[1].id, Timestamp(900))
        .unwrap();
    let history: Vec<String> = storage
        .tab_history(back, 10)
        .unwrap()
        .into_iter()
        .rev()
        .map(|entry| entry.url)
        .collect();
    assert_eq!(
        history,
        ["https://before.example/", "https://old-b.example/"]
    );
    let islands = storage.islands(WorkspaceId(1)).unwrap();
    assert_eq!(islands[1].name, "Alt");
    drop(storage);
    std::fs::remove_dir_all(directory).unwrap();
}
