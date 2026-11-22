use atoll_core::{IslandColor, IslandId, TabId, Timestamp, WorkspaceId};
use atoll_storage::prelude::*;
use atoll_storage::{ArchiveReason, Incoming, NewIsland, NewTab, Storage};
use atoll_sync::{Kind, Record, SyncId};

fn storage() -> Storage {
    Storage::open_in_memory().expect("in-memory database")
}

fn island(storage: &Storage, workspace: WorkspaceId, name: &str) -> IslandId {
    storage
        .create_island(NewIsland {
            workspace_id: workspace,
            name,
            color: IslandColor::Teal,
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
            title: Some(url),
            index: None,
            created_at: Timestamp(1),
        })
        .unwrap()
        .id
}

/// Everything `from` holds, taken over by `to`, as the sync thread does with what the server
/// hands out.
fn carry(from: &Storage, to: &Storage) {
    from.give_order_keys().unwrap();
    let incoming: Vec<Incoming> = from
        .sync_records()
        .unwrap()
        .into_iter()
        .map(|(kind, id, record)| Incoming {
            kind,
            id,
            record: Some(record),
        })
        .collect();
    to.transaction(|transaction| transaction.apply_sync(incoming))
        .unwrap();
}

type Layout = Vec<(String, Vec<(String, Vec<String>)>)>;

/// The workspaces, islands and tabs of a device as names, in the order the sidebar shows them.
fn layout(storage: &Storage) -> Layout {
    storage
        .workspaces()
        .unwrap()
        .into_iter()
        .map(|workspace| {
            let islands = storage
                .islands(workspace.id)
                .unwrap()
                .into_iter()
                .map(|island| {
                    let tabs = storage
                        .tabs_in_island(island.id)
                        .unwrap()
                        .into_iter()
                        .map(|tab| tab.url)
                        .collect();
                    (island.name, tabs)
                })
                .collect();
            (workspace.name, islands)
        })
        .collect()
}

fn sync_id_of_tab(storage: &Storage, tab: TabId) -> SyncId {
    storage
        .connection()
        .query_row("SELECT sync_id FROM tab WHERE id = ?1", [tab.0], |row| {
            row.get::<_, String>(0)
        })
        .unwrap()
        .parse()
        .unwrap()
}

#[test]
fn a_second_device_ends_up_with_the_same_layout_and_writes_nothing_down() {
    let laptop = storage();
    let work = laptop.create_workspace("Arbeit").unwrap();
    let docs = island(&laptop, work.id, "Doku");
    let news = island(&laptop, work.id, "News");
    tab(&laptop, docs, "https://a.example/", false);
    tab(&laptop, docs, "https://b.example/", false);
    tab(&laptop, news, "https://n.example/", false);
    laptop.create_workspace("Privat").unwrap();

    let phone = storage();
    carry(&laptop, &phone);
    assert_eq!(layout(&phone), layout(&laptop));
    assert!(
        phone.sync_outbox().unwrap().is_empty(),
        "nothing travels back"
    );
}

#[test]
fn private_tabs_are_never_carried() {
    let laptop = storage();
    let work = laptop.create_workspace("Arbeit").unwrap();
    let docs = island(&laptop, work.id, "Doku");
    tab(&laptop, docs, "https://open.example/", false);
    tab(&laptop, docs, "https://secret.example/", true);

    let kinds: Vec<Kind> = laptop
        .sync_records()
        .unwrap()
        .into_iter()
        .map(|(kind, _, _)| kind)
        .collect();
    assert_eq!(kinds.iter().filter(|kind| **kind == Kind::Tab).count(), 1);
}

#[test]
fn a_move_gives_only_the_moved_tab_a_new_key() {
    let laptop = storage();
    let work = laptop.create_workspace("Arbeit").unwrap();
    let docs = island(&laptop, work.id, "Doku");
    tab(&laptop, docs, "https://1.example/", false);
    tab(&laptop, docs, "https://2.example/", false);
    let third = tab(&laptop, docs, "https://3.example/", false);
    laptop.give_order_keys().unwrap();

    laptop.move_tab(third, docs, 0).unwrap();
    let changed = laptop.give_order_keys().unwrap();
    assert_eq!(changed, [(Kind::Tab, sync_id_of_tab(&laptop, third))]);
    assert!(
        laptop.give_order_keys().unwrap().is_empty(),
        "nothing left to fix"
    );
}

#[test]
fn moves_and_renames_arrive_in_the_same_order() {
    let laptop = storage();
    let work = laptop.create_workspace("Arbeit").unwrap();
    let docs = island(&laptop, work.id, "Doku");
    let news = island(&laptop, work.id, "News");
    tab(&laptop, docs, "https://1.example/", false);
    let second = tab(&laptop, docs, "https://2.example/", false);
    let phone = storage();
    carry(&laptop, &phone);

    laptop.move_tab(second, news, 0).unwrap();
    laptop.rename_island(docs, "Dokumente").unwrap();
    laptop.move_island(news, work.id, 0).unwrap();
    carry(&laptop, &phone);
    assert_eq!(layout(&phone), layout(&laptop));
}

#[test]
fn a_removal_takes_the_record_away_and_saves_private_tabs() {
    let laptop = storage();
    let work = laptop.create_workspace("Arbeit").unwrap();
    let docs = island(&laptop, work.id, "Doku");
    let keep = island(&laptop, work.id, "Bleibt");
    tab(&laptop, docs, "https://a.example/", false);
    tab(&laptop, keep, "https://k.example/", false);
    let phone = storage();
    carry(&laptop, &phone);
    let phone_docs = phone.islands(phone.workspaces().unwrap()[0].id).unwrap()[0].id;
    tab(&phone, phone_docs, "https://secret.example/", true);

    let docs_id: SyncId = laptop
        .connection()
        .query_row(
            "SELECT sync_id FROM island WHERE id = ?1",
            [docs.0],
            |row| row.get::<_, String>(0),
        )
        .unwrap()
        .parse()
        .unwrap();
    let applied = phone
        .transaction(|transaction| {
            transaction.apply_sync(vec![Incoming {
                kind: Kind::Island,
                id: docs_id,
                record: None,
            }])
        })
        .unwrap();
    assert_eq!(applied.changed, 1);
    let islands = phone.islands(phone.workspaces().unwrap()[0].id).unwrap();
    assert_eq!(islands.len(), 1);
    let urls: Vec<String> = phone
        .tabs_in_island(islands[0].id)
        .unwrap()
        .into_iter()
        .map(|tab| tab.url)
        .collect();
    assert_eq!(
        urls,
        ["https://k.example/", "https://secret.example/"],
        "the private tab moved to the island that stays"
    );
}

#[test]
fn children_that_arrive_before_their_parents_wait_for_them() {
    let laptop = storage();
    let work = laptop.create_workspace("Arbeit").unwrap();
    let docs = island(&laptop, work.id, "Doku");
    tab(&laptop, docs, "https://a.example/", false);
    laptop.give_order_keys().unwrap();
    let mut records = laptop.sync_records().unwrap();
    records.reverse();

    let phone = storage();
    let incoming = records
        .into_iter()
        .map(|(kind, id, record)| Incoming {
            kind,
            id,
            record: Some(record),
        })
        .collect();
    let applied = phone
        .transaction(|transaction| transaction.apply_sync(incoming))
        .unwrap();
    assert_eq!(applied.orphans, 0);
    assert_eq!(layout(&phone), layout(&laptop));
}

#[test]
fn a_tab_whose_island_never_comes_finds_a_home() {
    let phone = storage();
    let work = phone.create_workspace("Allgemein").unwrap();
    island(&phone, work.id, "Erste");
    let lost = Record::Tab(atoll_sync::model::Tab {
        island: SyncId::random(),
        url: "https://lost.example/".to_owned(),
        title: None,
        pinned: false,
        order: "V".to_owned(),
        last_used_at: None,
        created_at: 1,
    });
    let applied = phone
        .transaction(|transaction| {
            transaction.apply_sync(vec![Incoming {
                kind: Kind::Tab,
                id: SyncId::random(),
                record: Some(lost),
            }])
        })
        .unwrap();
    assert_eq!(applied.orphans, 1);
    assert_eq!(layout(&phone)[0].1[0].1, ["https://lost.example/"]);
}

#[test]
fn a_changed_address_is_reported_for_the_loaded_page() {
    let laptop = storage();
    let work = laptop.create_workspace("Arbeit").unwrap();
    let docs = island(&laptop, work.id, "Doku");
    let page = tab(&laptop, docs, "https://a.example/", false);
    let phone = storage();
    carry(&laptop, &phone);

    laptop
        .update_tab_address(page, "https://a.example/weiter")
        .unwrap();
    laptop.give_order_keys().unwrap();
    let id = sync_id_of_tab(&laptop, page);
    let record = laptop.sync_record(Kind::Tab, id).unwrap();
    let applied = phone
        .transaction(|transaction| {
            transaction.apply_sync(vec![Incoming {
                kind: Kind::Tab,
                id,
                record,
            }])
        })
        .unwrap();
    assert_eq!(applied.moved_tabs.len(), 1);
    assert_eq!(
        phone.tab(applied.moved_tabs[0]).unwrap().url,
        "https://a.example/weiter"
    );
}

#[test]
fn mirroring_makes_a_device_hold_exactly_the_server_state() {
    let server_side = storage();
    let work = server_side.create_workspace("Arbeit").unwrap();
    let docs = island(&server_side, work.id, "Doku");
    tab(&server_side, docs, "https://a.example/", false);
    let phone = storage();
    carry(&server_side, &phone);

    let phone_work = phone.workspaces().unwrap()[0].id;
    let phone_docs = phone.islands(phone_work).unwrap()[0].id;
    tab(&phone, phone_docs, "https://only-here.example/", false);
    tab(&phone, phone_docs, "https://secret.example/", true);
    phone.create_workspace("Nur hier").unwrap();

    server_side.give_order_keys().unwrap();
    let records = server_side.sync_records().unwrap();
    phone
        .transaction(|transaction| transaction.mirror_sync(records))
        .unwrap();
    let mut expected = layout(&server_side);
    expected[0].1[0]
        .1
        .push("https://secret.example/".to_owned());
    assert_eq!(
        layout(&phone),
        expected,
        "only the private tab stays besides the server's"
    );
}

#[test]
fn the_archive_travels_with_its_links() {
    let laptop = storage();
    let work = laptop.create_workspace("Arbeit").unwrap();
    let docs = island(&laptop, work.id, "Doku");
    tab(&laptop, docs, "https://stays.example/", false);
    let gone = tab(&laptop, docs, "https://archived.example/", false);
    laptop
        .archive_tabs(&[gone], Timestamp(5), ArchiveReason::Unused)
        .unwrap();

    let phone = storage();
    carry(&laptop, &phone);
    let archive = phone.archive().unwrap();
    assert_eq!(archive.len(), 1);
    let archived_island = &archive[0].islands[0];
    assert!(
        archived_island.island_id.is_some(),
        "the copy still points at the island on this device"
    );
    assert_eq!(archived_island.tabs[0].url, "https://archived.example/");
    let back = phone
        .restore_archived_tab(archived_island.tabs[0].id, Timestamp(9))
        .unwrap();
    assert_eq!(
        phone.tab(back).unwrap().island_id,
        phone.islands(phone.workspaces().unwrap()[0].id).unwrap()[0].id
    );
}

#[test]
fn a_workspace_already_on_the_server_can_be_adopted() {
    let server_side = storage();
    let work = server_side.create_workspace("Allgemein").unwrap();
    island(&server_side, work.id, "Server");
    let phone = storage();
    let local = phone.create_workspace("Allgemein").unwrap();
    island(&phone, local.id, "Telefon");

    server_side.give_order_keys().unwrap();
    let records = server_side.sync_records().unwrap();
    let (_, server_id, _) = records
        .iter()
        .find(|(kind, _, _)| *kind == Kind::Workspace)
        .unwrap()
        .clone();
    phone
        .transaction(|transaction| transaction.adopt_workspace(local.id, server_id))
        .unwrap();
    let incoming = records
        .into_iter()
        .map(|(kind, id, record)| Incoming {
            kind,
            id,
            record: Some(record),
        })
        .collect();
    phone
        .transaction(|transaction| transaction.apply_sync(incoming))
        .unwrap();
    let workspaces = phone.workspaces().unwrap();
    assert_eq!(
        workspaces.len(),
        1,
        "one workspace, not two of the same name"
    );
    let names: Vec<String> = phone
        .islands(workspaces[0].id)
        .unwrap()
        .into_iter()
        .map(|island| island.name)
        .collect();
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"Telefon".to_owned()) && names.contains(&"Server".to_owned()));
}
