use atoll_core::{IslandColor, TabId, Timestamp};
use atoll_storage::prelude::*;
use atoll_storage::{NewIsland, NewTab, Storage};

fn storage() -> Storage {
    Storage::open_in_memory().expect("in-memory database")
}

fn new_tab(island: atoll_core::IslandId, url: &str) -> NewTab<'_> {
    NewTab {
        private: false,
        island_id: island,
        url,
        title: None,
        index: None,
        created_at: Timestamp(1),
    }
}

fn new_island(workspace: atoll_core::WorkspaceId, name: &str) -> NewIsland<'_> {
    NewIsland {
        workspace_id: workspace,
        name,
        color: IslandColor::Blue,
        index: None,
        created_at: Timestamp(0),
    }
}

fn urls(storage: &Storage, island: atoll_core::IslandId) -> Vec<String> {
    storage
        .tabs_in_island(island)
        .unwrap()
        .into_iter()
        .map(|tab| tab.url)
        .collect()
}

#[test]
fn workspaces_are_ordered_and_renamable() {
    let storage = storage();
    let work = storage.create_workspace("Arbeit").unwrap();
    let private = storage.create_workspace("Privat").unwrap();
    storage.rename_workspace(work.id, "Job").unwrap();
    storage.move_workspace(private.id, 0).unwrap();

    let names: Vec<_> = storage
        .workspaces()
        .unwrap()
        .into_iter()
        .map(|w| w.name)
        .collect();
    assert_eq!(names, ["Privat", "Job"]);
}

#[test]
fn tabs_keep_their_order_when_inserted_and_moved() {
    let storage = storage();
    let workspace = storage.create_workspace("Arbeit").unwrap();
    let island = storage
        .create_island(new_island(workspace.id, "A"))
        .unwrap();
    let other = storage
        .create_island(new_island(workspace.id, "B"))
        .unwrap();

    let first = storage
        .create_tab(new_tab(island.id, "https://a.example/"))
        .unwrap();
    storage
        .create_tab(new_tab(island.id, "https://b.example/"))
        .unwrap();
    storage
        .create_tab(NewTab {
            private: false,
            index: Some(0),
            ..new_tab(island.id, "https://c.example/")
        })
        .unwrap();
    assert_eq!(
        urls(&storage, island.id),
        [
            "https://c.example/",
            "https://a.example/",
            "https://b.example/"
        ]
    );

    storage.move_tab(first.id, other.id, 0).unwrap();
    assert_eq!(
        urls(&storage, island.id),
        ["https://c.example/", "https://b.example/"]
    );
    assert_eq!(urls(&storage, other.id), ["https://a.example/"]);

    let positions: Vec<_> = storage
        .tabs_in_island(island.id)
        .unwrap()
        .iter()
        .map(|t| t.position)
        .collect();
    assert_eq!(positions, [0, 1]);
}

#[test]
fn archived_islands_leave_the_workspace_and_come_back_at_the_end() {
    let storage = storage();
    let workspace = storage.create_workspace("Arbeit").unwrap();
    let a = storage
        .create_island(new_island(workspace.id, "A"))
        .unwrap();
    let b = storage
        .create_island(new_island(workspace.id, "B"))
        .unwrap();
    storage
        .create_tab(new_tab(a.id, "https://a.example/"))
        .unwrap();

    storage.archive_island(a.id, Timestamp(10)).unwrap();
    assert_eq!(storage.islands(workspace.id).unwrap().len(), 1);
    assert!(storage.tabs_in_workspace(workspace.id).unwrap().is_empty());
    assert_eq!(
        storage.all_tabs_in_workspace(workspace.id).unwrap().len(),
        1
    );

    let archived = storage.archived_islands().unwrap();
    assert_eq!(archived.len(), 1);
    assert_eq!(archived[0].tab_count, 1);
    assert_eq!(archived[0].workspace_name, "Arbeit");

    storage.restore_island(a.id).unwrap();
    let order: Vec<_> = storage
        .islands(workspace.id)
        .unwrap()
        .into_iter()
        .map(|i| i.id)
        .collect();
    assert_eq!(order, [b.id, a.id]);
}

#[test]
fn deleting_a_workspace_removes_its_islands_and_tabs() {
    let storage = storage();
    let workspace = storage.create_workspace("Arbeit").unwrap();
    let island = storage
        .create_island(new_island(workspace.id, "A"))
        .unwrap();
    let tab = storage
        .create_tab(new_tab(island.id, "https://a.example/"))
        .unwrap();

    storage.delete_workspace(workspace.id).unwrap();
    assert!(storage.tab(tab.id).is_err());
    assert!(storage.island(island.id).is_err());
}

#[test]
fn touching_a_tab_marks_its_island_as_used() {
    let storage = storage();
    let workspace = storage.create_workspace("Arbeit").unwrap();
    let island = storage
        .create_island(new_island(workspace.id, "A"))
        .unwrap();
    let tab = storage
        .create_tab(new_tab(island.id, "https://a.example/"))
        .unwrap();

    storage.touch_tab(tab.id, Timestamp(42)).unwrap();
    assert_eq!(
        storage.tab(tab.id).unwrap().last_used_at,
        Some(Timestamp(42))
    );
    assert_eq!(
        storage.island(island.id).unwrap().last_used_at,
        Some(Timestamp(42))
    );
    assert!(
        storage
            .islands_unused_since(Timestamp(40))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        storage.islands_unused_since(Timestamp(50)).unwrap().len(),
        1
    );
}

#[test]
fn favicons_are_shared_per_host() {
    let storage = storage();
    let first = storage
        .upsert_favicon("example.com", &[1, 2], Timestamp(1))
        .unwrap();
    let second = storage
        .upsert_favicon("example.com", &[3], Timestamp(2))
        .unwrap();
    assert_eq!(first, second);
    assert_eq!(storage.favicon(first).unwrap(), Some(vec![3]));
    assert_eq!(storage.favicons(&[first]).unwrap(), vec![(first, vec![3])]);
    assert_eq!(
        storage.favicon_hosts().unwrap(),
        vec![(first, "example.com".to_owned())]
    );
}

#[test]
fn history_acts_as_a_replacement_back_list() {
    let storage = storage();
    let workspace = storage.create_workspace("Arbeit").unwrap();
    let island = storage
        .create_island(new_island(workspace.id, "A"))
        .unwrap();
    let tab = storage
        .create_tab(new_tab(island.id, "https://a.example/"))
        .unwrap();

    storage
        .record_visit(tab.id, "https://a.example/", None, Timestamp(1))
        .unwrap();
    storage
        .record_visit(tab.id, "https://a.example/", None, Timestamp(2))
        .unwrap();
    storage
        .record_visit(tab.id, "https://b.example/", Some("B"), Timestamp(3))
        .unwrap();
    assert_eq!(storage.tab_history(tab.id, 10).unwrap().len(), 2);

    assert_eq!(
        storage.step_back(tab.id, "https://b.example/").unwrap(),
        Some("https://a.example/".into())
    );
    assert_eq!(
        storage.step_back(tab.id, "https://a.example/").unwrap(),
        None
    );
}

#[test]
fn transactions_roll_back_on_error() {
    let storage = storage();
    let result = storage.transaction(|transaction| {
        transaction.create_workspace("Temporär")?;
        transaction.tab(TabId(999))
    });
    assert!(result.is_err());
    assert!(storage.workspaces().unwrap().is_empty());
}

#[test]
fn settings_round_trip() {
    let storage = storage();
    assert_eq!(storage.setting("missing").unwrap(), None);
    storage.set_setting("active_workspace", "7").unwrap();
    storage.set_setting("active_workspace", "8").unwrap();
    assert_eq!(storage.setting_i64("active_workspace").unwrap(), Some(8));
    storage.remove_setting("active_workspace").unwrap();
    assert_eq!(storage.setting("active_workspace").unwrap(), None);
}

#[test]
fn file_database_uses_write_ahead_logging() {
    let directory = std::env::temp_dir().join(format!("atoll-storage-test-{}", std::process::id()));
    let path = directory.join("atoll.db");
    {
        let storage = Storage::open(&path).unwrap();
        storage.create_workspace("Arbeit").unwrap();
        let mode: String = storage
            .connection()
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
    }
    let reopened = Storage::open(&path).unwrap();
    assert_eq!(reopened.workspaces().unwrap().len(), 1);
    drop(reopened);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn new_islands_count_as_used_when_created() {
    let storage = storage();
    let workspace = storage.create_workspace("Arbeit").unwrap();
    let island = storage
        .create_island(NewIsland {
            created_at: Timestamp(100),
            ..new_island(workspace.id, "Neu")
        })
        .unwrap();
    assert_eq!(island.last_used_at, Some(Timestamp(100)));
    assert!(
        storage
            .islands_unused_since(Timestamp(100))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        storage.islands_unused_since(Timestamp(101)).unwrap().len(),
        1
    );
}

#[test]
fn private_tabs_keep_nothing_and_go_away_with_the_run() {
    let storage = Storage::open_in_memory().unwrap();
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
    let open = storage
        .create_tab(NewTab {
            private: false,
            island_id: island.id,
            url: "https://example.org/",
            title: Some("Offen"),
            index: None,
            created_at: Timestamp(1),
        })
        .unwrap();
    let hidden = storage
        .create_tab(NewTab {
            private: true,
            island_id: island.id,
            url: "",
            title: None,
            index: None,
            created_at: Timestamp(2),
        })
        .unwrap();
    assert!(!open.private);
    assert!(hidden.private);
    assert_eq!(storage.tabs_in_island(island.id).unwrap().len(), 2);

    assert_eq!(storage.clear_private_tabs().unwrap(), 1);
    let left = storage.tabs_in_island(island.id).unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].id, open.id, "only the ordinary tab survives a run");
    assert_eq!(
        storage.clear_private_tabs().unwrap(),
        0,
        "a second run finds nothing left"
    );
}
