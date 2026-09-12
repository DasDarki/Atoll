use super::*;
use crate::parse;

fn target() -> ImportTarget {
    ImportTarget {
        islands: vec![ExistingIsland {
            id: IslandId(1),
            name: "Recherche".into(),
        }],
        tabs: vec![ExistingTab {
            id: TabId(10),
            url: "https://example.com".into(),
        }],
    }
}

fn options(duplicates: DuplicatePolicy, island_names: IslandNamePolicy) -> ImportOptions {
    ImportOptions {
        duplicates,
        island_names,
        ..ImportOptions::default()
    }
}

const LIST: &str = "https://loose.example\n--- recherche ---\nhttps://example.com/\nhttps://new.example\nhttps://new.example/\n--- Neu ---\nhttps://other.example\nquatsch\n";

#[test]
fn keep_imports_everything_and_merges_islands_by_name() {
    let plan = plan_import(
        &parse(LIST),
        &target(),
        &options(DuplicatePolicy::Keep, IslandNamePolicy::Merge),
    );
    let destinations: Vec<_> = plan
        .islands
        .iter()
        .map(|island| (island.name.as_str(), island.destination))
        .collect();
    assert_eq!(
        destinations,
        [
            ("Importiert", IslandDestination::New),
            ("recherche", IslandDestination::Existing(IslandId(1))),
            ("Neu", IslandDestination::New),
        ]
    );
    assert_eq!(plan.created_count(), 5);
    assert!(plan.skipped.is_empty());
    assert_eq!(plan.invalid.len(), 1);
    assert_eq!(plan.new_island_count(), 2);
}

#[test]
fn skip_leaves_out_existing_and_repeated_addresses() {
    let plan = plan_import(
        &parse(LIST),
        &target(),
        &options(DuplicatePolicy::Skip, IslandNamePolicy::Merge),
    );
    assert_eq!(plan.created_count(), 3);
    let reasons: Vec<_> = plan.skipped.iter().map(|skipped| skipped.reason).collect();
    assert_eq!(
        reasons,
        [SkipReason::AlreadyPresent, SkipReason::RepeatedInList]
    );
}

#[test]
fn merge_moves_existing_tabs_into_the_imported_island() {
    let plan = plan_import(
        &parse(LIST),
        &target(),
        &options(DuplicatePolicy::Merge, IslandNamePolicy::Merge),
    );
    assert_eq!(plan.moved_count(), 1);
    assert_eq!(
        plan.islands[1].entries[0],
        PlannedEntry::Move {
            tab: TabId(10),
            url: "https://example.com/".into()
        }
    );
    assert_eq!(plan.created_count(), 3);
    assert_eq!(plan.skipped.len(), 1);
}

#[test]
fn create_new_never_reuses_islands() {
    let list =
        parse("--- Recherche ---\nhttps://a.example\n--- Recherche ---\nhttps://b.example\n");
    let plan = plan_import(
        &list,
        &target(),
        &options(DuplicatePolicy::Keep, IslandNamePolicy::CreateNew),
    );
    assert_eq!(plan.islands.len(), 2);
    assert!(
        plan.islands
            .iter()
            .all(|island| island.destination == IslandDestination::New)
    );
}

#[test]
fn repeated_markers_merge_within_the_list() {
    let list = parse("--- A ---\nhttps://a.example\n--- B ---\n--- A ---\nhttps://b.example\n");
    let plan = plan_import(&list, &ImportTarget::default(), &ImportOptions::default());
    assert_eq!(plan.islands.len(), 2);
    assert_eq!(plan.islands[0].entries.len(), 2);
    assert!(plan.islands[1].entries.is_empty());
}

#[test]
fn custom_fallback_island_name_is_used() {
    let list = parse("https://a.example\n");
    let options = ImportOptions {
        fallback_island: "Aus Opera".into(),
        ..ImportOptions::default()
    };
    let plan = plan_import(&list, &ImportTarget::default(), &options);
    assert_eq!(plan.islands[0].name, "Aus Opera");
}

#[test]
fn counted_islands_are_named_one_two_three() {
    let list = parse(
        "--- Arbeit ---\nhttps://a.example/\n--- Freizeit ---\nhttps://b.example/\nhttps://c.example/",
    );
    let target = ImportTarget {
        islands: Vec::new(),
        tabs: Vec::new(),
    };
    let plan = plan_import(
        &list,
        &target,
        &ImportOptions {
            naming: IslandNaming::Numbered,
            ..ImportOptions::default()
        },
    );
    let names: Vec<&str> = plan
        .islands
        .iter()
        .map(|island| island.name.as_str())
        .collect();
    assert_eq!(
        names,
        vec!["1", "2"],
        "the markers of the list are replaced"
    );
    assert_eq!(plan.islands[1].entries.len(), 2);
}

#[test]
fn counting_also_names_the_island_of_the_addresses_without_a_marker() {
    let list = parse("https://a.example/\n--- Arbeit ---\nhttps://b.example/");
    let target = ImportTarget {
        islands: Vec::new(),
        tabs: Vec::new(),
    };
    let plan = plan_import(
        &list,
        &target,
        &ImportOptions {
            naming: IslandNaming::Numbered,
            fallback_island: "Importiert".to_owned(),
            ..ImportOptions::default()
        },
    );
    let names: Vec<&str> = plan
        .islands
        .iter()
        .map(|island| island.name.as_str())
        .collect();
    assert_eq!(names, vec!["1", "2"]);
}
