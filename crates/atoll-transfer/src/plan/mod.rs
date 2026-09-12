use std::collections::HashMap;

use atoll_core::address::normalize;
use atoll_core::{IslandId, TabId};

use crate::parse::{InvalidLine, ParsedList};

/// How addresses that already exist in the target are treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DuplicatePolicy {
    /// Import every address, even if a tab with it exists already.
    #[default]
    Keep,
    /// Leave out addresses that exist already or appear twice in the list.
    Skip,
    /// Move the existing tab into the imported island instead of creating a second one.
    Merge,
}

/// How imported islands relate to existing islands with the same name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IslandNamePolicy {
    /// Add the addresses to the existing island of the same name.
    #[default]
    Merge,
    /// Always create a new island.
    CreateNew,
}

/// Where the names of the islands come from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum IslandNaming {
    /// The markers of the list, and the fallback island for everything before the first one.
    #[default]
    FromList,
    /// Counted up, starting at one, in the order the islands appear in the list.
    Numbered,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportOptions {
    pub duplicates: DuplicatePolicy,
    pub island_names: IslandNamePolicy,
    pub naming: IslandNaming,
    /// The island for addresses that appear before the first marker.
    pub fallback_island: String,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            duplicates: DuplicatePolicy::default(),
            island_names: IslandNamePolicy::default(),
            naming: IslandNaming::default(),
            fallback_island: String::from("Importiert"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExistingIsland {
    pub id: IslandId,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExistingTab {
    pub id: TabId,
    pub url: String,
}

/// The current content of the workspace an import goes into.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportTarget {
    pub islands: Vec<ExistingIsland>,
    pub tabs: Vec<ExistingTab>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IslandDestination {
    Existing(IslandId),
    New,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannedEntry {
    Create { url: String },
    Move { tab: TabId, url: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedIsland {
    pub destination: IslandDestination,
    pub name: String,
    pub entries: Vec<PlannedEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    AlreadyPresent,
    RepeatedInList,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedUrl {
    pub url: String,
    pub island: String,
    pub reason: SkipReason,
}

/// Everything an import will change, computed before anything is written.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportPlan {
    pub islands: Vec<PlannedIsland>,
    pub skipped: Vec<SkippedUrl>,
    pub invalid: Vec<InvalidLine>,
}

impl ImportPlan {
    pub fn created_count(&self) -> usize {
        self.count_entries(|entry| matches!(entry, PlannedEntry::Create { .. }))
    }

    pub fn moved_count(&self) -> usize {
        self.count_entries(|entry| matches!(entry, PlannedEntry::Move { .. }))
    }

    pub fn new_island_count(&self) -> usize {
        self.islands
            .iter()
            .filter(|island| island.destination == IslandDestination::New)
            .count()
    }

    fn count_entries(&self, predicate: impl Fn(&PlannedEntry) -> bool) -> usize {
        self.islands
            .iter()
            .flat_map(|island| &island.entries)
            .filter(|entry| predicate(entry))
            .count()
    }
}

#[derive(Clone, Copy)]
enum Seen {
    Existing(TabId),
    Moved,
    Planned,
}

/// Decides where every address of `list` ends up in `target` under the given options.
pub fn plan_import(
    list: &ParsedList,
    target: &ImportTarget,
    options: &ImportOptions,
) -> ImportPlan {
    let mut plan = ImportPlan {
        invalid: list.invalid.clone(),
        ..ImportPlan::default()
    };
    let mut seen: HashMap<String, Seen> = target
        .tabs
        .iter()
        .map(|tab| (normalize(&tab.url), Seen::Existing(tab.id)))
        .collect();
    let mut planned_by_name: HashMap<String, usize> = HashMap::new();

    for (number, group) in list.groups.iter().enumerate() {
        let name = match options.naming {
            IslandNaming::Numbered => (number + 1).to_string(),
            IslandNaming::FromList => group
                .name
                .clone()
                .unwrap_or_else(|| options.fallback_island.clone()),
        };
        let index = island_index(&mut plan, &mut planned_by_name, target, options, &name);

        for url in &group.urls {
            let key = normalize(url);
            let previous = seen.get(&key).copied();
            let entry = match (options.duplicates, previous) {
                (DuplicatePolicy::Keep, _) | (_, None) => {
                    seen.entry(key).or_insert(Seen::Planned);
                    Some(PlannedEntry::Create { url: url.clone() })
                }
                (DuplicatePolicy::Merge, Some(Seen::Existing(tab))) => {
                    seen.insert(key, Seen::Moved);
                    Some(PlannedEntry::Move {
                        tab,
                        url: url.clone(),
                    })
                }
                (_, Some(previous)) => {
                    plan.skipped.push(SkippedUrl {
                        url: url.clone(),
                        island: name.clone(),
                        reason: match previous {
                            Seen::Existing(_) => SkipReason::AlreadyPresent,
                            Seen::Moved | Seen::Planned => SkipReason::RepeatedInList,
                        },
                    });
                    None
                }
            };
            if let Some(entry) = entry {
                plan.islands[index].entries.push(entry);
            }
        }
    }

    plan
}

fn island_index(
    plan: &mut ImportPlan,
    planned_by_name: &mut HashMap<String, usize>,
    target: &ImportTarget,
    options: &ImportOptions,
    name: &str,
) -> usize {
    let key = name.to_lowercase();
    if options.island_names == IslandNamePolicy::Merge
        && let Some(index) = planned_by_name.get(&key)
    {
        return *index;
    }

    let destination = match options.island_names {
        IslandNamePolicy::Merge => target
            .islands
            .iter()
            .find(|island| island.name.to_lowercase() == key)
            .map_or(IslandDestination::New, |island| {
                IslandDestination::Existing(island.id)
            }),
        IslandNamePolicy::CreateNew => IslandDestination::New,
    };
    plan.islands.push(PlannedIsland {
        destination,
        name: name.to_owned(),
        entries: Vec::new(),
    });
    let index = plan.islands.len() - 1;
    planned_by_name.insert(key, index);
    index
}

#[cfg(test)]
mod tests;
