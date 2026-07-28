use atoll_core::Timestamp;
use atoll_storage::Storage;
use atoll_storage::prelude::*;

#[test]
fn allowed_sites_can_be_added_and_removed() {
    let storage = Storage::open_in_memory().expect("in-memory database");
    assert!(storage.adblock_allowed_sites().unwrap().is_empty());

    storage
        .set_adblock_site_allowed("news.example", true, Timestamp(1))
        .unwrap();
    storage
        .set_adblock_site_allowed("example.org", true, Timestamp(2))
        .unwrap();
    storage
        .set_adblock_site_allowed("example.org", true, Timestamp(3))
        .unwrap();
    assert_eq!(
        storage.adblock_allowed_sites().unwrap(),
        ["example.org", "news.example"]
    );

    storage
        .set_adblock_site_allowed("example.org", false, Timestamp(4))
        .unwrap();
    assert_eq!(storage.adblock_allowed_sites().unwrap(), ["news.example"]);
}
