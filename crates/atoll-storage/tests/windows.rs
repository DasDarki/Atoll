use atoll_core::WindowPlacement;
use atoll_storage::Storage;
use atoll_storage::prelude::*;

fn storage() -> Storage {
    Storage::open_in_memory().expect("in-memory database")
}

#[test]
fn windows_are_remembered_in_opening_order() {
    let storage = storage();
    let work = storage.create_workspace("Arbeit").unwrap();
    let home = storage.create_workspace("Privat").unwrap();
    let first = storage.create_window(home.id).unwrap();
    let second = storage.create_window(work.id).unwrap();

    let windows = storage.windows().unwrap();
    assert_eq!(
        windows.iter().map(|window| window.id).collect::<Vec<_>>(),
        [first.id, second.id]
    );
    assert_eq!(windows[0].workspace_id, home.id);
    assert_eq!(windows[0].size, None);
}

#[test]
fn placements_and_workspaces_can_change() {
    let storage = storage();
    let work = storage.create_workspace("Arbeit").unwrap();
    let home = storage.create_workspace("Privat").unwrap();
    let window = storage.create_window(work.id).unwrap();
    let placement = WindowPlacement {
        position: Some((40, -20)),
        size: Some((1280, 800)),
        maximized: true,
        ..window
    };
    storage.update_window_placement(&placement).unwrap();
    storage.set_window_workspace(placement.id, home.id).unwrap();

    let stored = &storage.windows().unwrap()[0];
    assert_eq!(stored.position, Some((40, -20)));
    assert_eq!(stored.size, Some((1280, 800)));
    assert!(stored.maximized);
    assert_eq!(stored.workspace_id, home.id);

    storage.delete_window(placement.id).unwrap();
    assert!(storage.windows().unwrap().is_empty());
}

#[test]
fn a_workspace_is_shown_in_one_window_at_most() {
    let storage = storage();
    let work = storage.create_workspace("Arbeit").unwrap();
    storage.create_window(work.id).unwrap();
    assert!(storage.create_window(work.id).is_err());
}

#[test]
fn deleting_a_workspace_forgets_its_window() {
    let storage = storage();
    let work = storage.create_workspace("Arbeit").unwrap();
    storage.create_workspace("Privat").unwrap();
    storage.create_window(work.id).unwrap();
    storage.delete_workspace(work.id).unwrap();
    assert!(storage.windows().unwrap().is_empty());
}
