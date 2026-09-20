use atoll_core::Timestamp;
use atoll_storage::prelude::*;
use atoll_storage::{AdblockRule, Storage};

#[test]
fn rules_for_sites_and_addresses_live_next_to_the_one_for_everything() {
    let storage = Storage::open_in_memory().expect("in-memory database");
    assert!(storage.adblock_rules().unwrap().is_empty());

    let everywhere = AdblockRule {
        scope: "global".to_owned(),
        pattern: String::new(),
        popups: Some(true),
        ..AdblockRule::default()
    };
    let site = AdblockRule {
        scope: "site".to_owned(),
        pattern: "news.example".to_owned(),
        active: Some(false),
        ..AdblockRule::default()
    };
    let page = AdblockRule {
        scope: "page".to_owned(),
        pattern: "https://example.org/kasse".to_owned(),
        popups: Some(false),
        ..AdblockRule::default()
    };
    for rule in [&everywhere, &site, &page] {
        storage.set_adblock_rule(rule, Timestamp(1)).unwrap();
    }
    assert_eq!(
        storage.adblock_rules().unwrap(),
        vec![everywhere.clone(), page, site]
    );

    storage.clear_adblock_exceptions().unwrap();
    assert_eq!(storage.adblock_rules().unwrap(), vec![everywhere]);
}
