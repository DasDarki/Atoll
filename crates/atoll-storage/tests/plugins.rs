use atoll_core::Timestamp;
use atoll_storage::prelude::*;
use atoll_storage::{PluginRecord, Storage};

fn storage() -> Storage {
    Storage::open_in_memory().expect("in-memory database")
}

fn record(id: &str, permissions: &[&str]) -> PluginRecord {
    PluginRecord {
        id: id.to_owned(),
        version: "1.0".to_owned(),
        enabled: true,
        permissions: permissions.iter().map(|name| (*name).to_owned()).collect(),
        source: "folder".to_owned(),
        installed_at: Timestamp(1),
    }
}

#[test]
fn installed_extensions_keep_their_granted_permissions() {
    let storage = storage();
    storage
        .save_plugin(&record("reddit-tweaks", &["pages", "storage"]))
        .unwrap();
    storage.save_plugin(&record("notes", &[])).unwrap();

    let plugins = storage.plugins().unwrap();
    assert_eq!(plugins.len(), 2);
    assert_eq!(plugins[0].id, "notes", "sorted by name");
    assert!(plugins[0].permissions.is_empty());
    let tweaks = storage.plugin("reddit-tweaks").unwrap().unwrap();
    assert_eq!(tweaks.permissions, ["pages", "storage"]);
    assert!(tweaks.enabled);
    assert_eq!(storage.plugin("nothing").unwrap(), None);
}

#[test]
fn saving_again_updates_the_version_and_keeps_the_data() {
    let storage = storage();
    storage.save_plugin(&record("notes", &["storage"])).unwrap();
    storage
        .set_plugin_value("notes", "draft", Some("hallo"))
        .unwrap();

    let mut newer = record("notes", &["storage", "tabs"]);
    newer.version = "2.0".to_owned();
    newer.enabled = false;
    storage.save_plugin(&newer).unwrap();

    let stored = storage.plugin("notes").unwrap().unwrap();
    assert_eq!(stored.version, "2.0");
    assert_eq!(stored.permissions, ["storage", "tabs"]);
    assert!(!stored.enabled);
    assert_eq!(
        storage.plugin_value("notes", "draft").unwrap().as_deref(),
        Some("hallo")
    );
}

#[test]
fn every_extension_has_its_own_data() {
    let storage = storage();
    storage.save_plugin(&record("a", &["storage"])).unwrap();
    storage.save_plugin(&record("b", &["storage"])).unwrap();
    storage.set_plugin_value("a", "key", Some("von a")).unwrap();
    storage.set_plugin_value("b", "key", Some("von b")).unwrap();

    assert_eq!(
        storage.plugin_value("a", "key").unwrap().as_deref(),
        Some("von a")
    );
    assert_eq!(storage.plugin_keys("b").unwrap(), ["key"]);

    storage.set_plugin_value("a", "key", None).unwrap();
    assert_eq!(storage.plugin_value("a", "key").unwrap(), None);
    assert_eq!(
        storage.plugin_value("b", "key").unwrap().as_deref(),
        Some("von b"),
        "removing in one extension leaves the other alone"
    );
}

#[test]
fn removing_an_extension_removes_its_data() {
    let storage = storage();
    storage.save_plugin(&record("notes", &["storage"])).unwrap();
    storage
        .set_plugin_value("notes", "draft", Some("x"))
        .unwrap();
    storage.set_plugin_enabled("notes", false).unwrap();
    assert!(!storage.plugin("notes").unwrap().unwrap().enabled);

    storage.remove_plugin("notes").unwrap();
    assert_eq!(storage.plugin("notes").unwrap(), None);
    assert!(storage.plugin_keys("notes").unwrap().is_empty());
}
