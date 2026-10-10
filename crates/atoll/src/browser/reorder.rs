//! Dragging an island or a tab to another place in the sidebar.
//!
//! The row that is being dragged keeps the pointer, so the other rows never hear about the
//! pointer themselves. They are told where it is instead and report when it stands over them,
//! which lands here as a target and a side. This turns that into the place the row would get
//! and shows a line there, so the drop holds no surprise.
//!
//! A tab lands where the line stands: inside its island, in another one, or at the top of an
//! island whose header it was dropped on, which is the only way into one that is collapsed.

use atoll_core::{IslandId, TabId};
use slint::{ComponentHandle, Model};

use super::Browser;
use crate::ui::SidebarState;

/// No island at all, the value the sidebar shows no line for.
const NOTHING: i32 = -2;

/// Below the last island, where no island follows to draw the line at.
const AT_THE_END: i32 = -1;

/// An island on its way to another place.
#[derive(Debug, Default, Clone, Copy)]
pub struct IslandDrag {
    island: Option<IslandId>,
    /// Where it would land, counted among the islands without it.
    insert: Option<usize>,
}

/// A tab on its way to another place.
#[derive(Debug, Default, Clone)]
pub struct TabDrag {
    tab: Option<TabId>,
    /// The island it would land in and its place among that island's tabs.
    target: Option<(IslandId, usize)>,
}

/// What the pointer stands over while a tab is dragged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Over {
    /// A tab, in its lower half when the flag is set.
    Tab(TabId, bool),
    /// The header of an island, which means the top of it.
    Island(IslandId),
}

impl Browser {
    /// An island is being dragged.
    pub fn island_drag_started(&self, island: IslandId) {
        *self.island_drag.borrow_mut() = IslandDrag {
            island: Some(island),
            insert: None,
        };
        if let Some(window) = self.window() {
            let state = window.global::<SidebarState>();
            state.set_island_drag_id(island.0 as i32);
            state.set_island_drop_before(NOTHING);
        }
    }

    /// The pointer stands over `target`, in its lower half when `after` is set.
    pub fn island_drag_hover(&self, target: IslandId, after: bool) {
        let Some(dragged) = self.island_drag.borrow().island else {
            return;
        };
        let order: Vec<IslandId> = self
            .session
            .borrow()
            .islands
            .iter()
            .map(|entry| entry.island.id)
            .collect();
        let placement = placement(&order, dragged, target, after);
        self.island_drag.borrow_mut().insert = placement.map(|(insert, _)| insert);
        if let Some(window) = self.window() {
            window
                .global::<SidebarState>()
                .set_island_drop_before(placement.map_or(NOTHING, |(_, before)| before));
        }
    }

    /// The island was let go. It moves if the place is another one than it had.
    pub fn island_drag_ended(&self) {
        let drag = std::mem::take(&mut *self.island_drag.borrow_mut());
        if let Some(window) = self.window() {
            let state = window.global::<SidebarState>();
            state.set_island_drag_id(AT_THE_END);
            state.set_island_drop_before(NOTHING);
        }
        if let (Some(island), Some(insert)) = (drag.island, drag.insert) {
            self.move_island_to(island, insert);
        }
    }

    /// A tab is being dragged.
    pub fn tab_drag_started(&self, tab: TabId) {
        *self.tab_drag.borrow_mut() = TabDrag {
            tab: Some(tab),
            target: None,
        };
        if let Some(window) = self.window() {
            let state = window.global::<SidebarState>();
            state.set_tab_drag_id(tab.0 as i32);
            state.set_tab_drop_row(NOTHING);
        }
    }

    /// The pointer stands over the tab `target`, in its lower half when `after` is set.
    pub fn tab_drag_over_tab(&self, target: TabId, after: bool) {
        self.tab_drag_over(Over::Tab(target, after));
    }

    /// The pointer stands over the header of `island`.
    pub fn tab_drag_over_island(&self, island: IslandId) {
        self.tab_drag_over(Over::Island(island));
    }

    fn tab_drag_over(&self, over: Over) {
        let Some(dragged) = self.tab_drag.borrow().tab else {
            return;
        };
        let target = {
            let session = self.session.borrow();
            let islands = islands_with_tabs(&session);
            placement_of_tab(&islands, dragged, over)
        };
        self.tab_drag.borrow_mut().target = target;
        if let Some(window) = self.window() {
            let row = target.map_or(NOTHING, |(island, index)| {
                self.drop_row_of(dragged, island, index)
            });
            window.global::<SidebarState>().set_tab_drop_row(row);
        }
    }

    /// The tab was let go inside the sidebar. It moves if the place is another one than it had.
    pub(super) fn tab_drag_dropped(&self) {
        let drag = std::mem::take(&mut *self.tab_drag.borrow_mut());
        if let Some(window) = self.window() {
            let state = window.global::<SidebarState>();
            state.set_tab_drag_id(NOTHING);
            state.set_tab_drop_row(NOTHING);
        }
        if let (Some(tab), Some((island, index))) = (drag.tab, drag.target) {
            self.move_tab_to(tab, island, index);
        }
    }

    /// The tab was let go where it cannot land, so the line goes away.
    pub(super) fn forget_tab_drag(&self) {
        *self.tab_drag.borrow_mut() = TabDrag::default();
        if let Some(window) = self.window() {
            let state = window.global::<SidebarState>();
            state.set_tab_drag_id(NOTHING);
            state.set_tab_drop_row(NOTHING);
        }
    }

    /// Where the line is drawn: above the row that would follow the tab, or below the last row of
    /// the island when nothing follows. The dragged tab is passed over while counting, because the
    /// place was counted without it.
    fn drop_row_of(&self, dragged: TabId, island: IslandId, index: usize) -> i32 {
        let mut seen = 0;
        let mut after = None;
        for (row, entry) in self.sidebar.rows.iter().enumerate() {
            if entry.island_id != island.0 as i32 {
                continue;
            }
            if entry.kind != crate::ui::RowKind::Tab || entry.tab_id == dragged.0 as i32 {
                after = Some(row as i32 + 1);
                continue;
            }
            if seen == index {
                return row as i32;
            }
            seen += 1;
            after = Some(row as i32 + 1);
        }
        after.unwrap_or(NOTHING)
    }
}

/// The islands of the session with the tabs they hold, in the order the sidebar shows them.
fn islands_with_tabs(session: &super::session::Session) -> Vec<(IslandId, Vec<TabId>)> {
    session
        .islands
        .iter()
        .map(|entry| {
            (
                entry.island.id,
                entry.tabs.iter().map(|tab| tab.id).collect(),
            )
        })
        .collect()
}

/// The island a dragged tab would land in and its place among that island's tabs, counted
/// without the tab itself. Answers with nothing when it would stay where it is.
fn placement_of_tab(
    islands: &[(IslandId, Vec<TabId>)],
    dragged: TabId,
    over: Over,
) -> Option<(IslandId, usize)> {
    let (island, index) = match over {
        Over::Tab(target, after) => {
            if target == dragged {
                return None;
            }
            let (island, tabs) = islands.iter().find(|(_, tabs)| tabs.contains(&target))?;
            let without: Vec<TabId> = tabs.iter().copied().filter(|tab| *tab != dragged).collect();
            let at = without.iter().position(|tab| *tab == target)?;
            (*island, at + usize::from(after))
        }
        Over::Island(island) => (island, 0),
    };
    let stays = islands
        .iter()
        .find(|(id, _)| *id == island)
        .is_some_and(|(_, tabs)| tabs.get(index) == Some(&dragged));
    if stays {
        return None;
    }
    Some((island, index))
}

/// Where the dragged island would land, and the island the line is drawn above. Answers with
/// nothing when it would stay where it is.
fn placement(
    order: &[IslandId],
    dragged: IslandId,
    target: IslandId,
    after: bool,
) -> Option<(usize, i32)> {
    let without: Vec<IslandId> = order
        .iter()
        .copied()
        .filter(|island| *island != dragged)
        .collect();
    let target_index = without.iter().position(|island| *island == target)?;
    let insert = if after {
        target_index + 1
    } else {
        target_index
    };
    let mut moved = without.clone();
    moved.insert(insert, dragged);
    if moved == order {
        return None;
    }
    let before = without
        .get(insert)
        .map_or(AT_THE_END, |island| island.0 as i32);
    Some((insert, before))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_tabs(entries: &[(i64, &[i64])]) -> Vec<(IslandId, Vec<TabId>)> {
        entries
            .iter()
            .map(|(island, tabs)| {
                (
                    IslandId(*island),
                    tabs.iter().map(|tab| TabId(*tab)).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn a_tab_lands_above_or_below_the_tab_it_is_dropped_on() {
        let islands = with_tabs(&[(1, &[10, 11, 12])]);
        assert_eq!(
            placement_of_tab(&islands, TabId(12), Over::Tab(TabId(10), false)),
            Some((IslandId(1), 0)),
            "above the first tab"
        );
        assert_eq!(
            placement_of_tab(&islands, TabId(12), Over::Tab(TabId(10), true)),
            Some((IslandId(1), 1)),
            "below it, counted without the dragged tab"
        );
        assert_eq!(
            placement_of_tab(&islands, TabId(10), Over::Tab(TabId(12), true)),
            Some((IslandId(1), 2)),
            "to the very end"
        );
    }

    #[test]
    fn a_tab_dropped_where_it_already_is_stays() {
        let islands = with_tabs(&[(1, &[10, 11, 12])]);
        assert_eq!(
            placement_of_tab(&islands, TabId(11), Over::Tab(TabId(10), true)),
            None
        );
        assert_eq!(
            placement_of_tab(&islands, TabId(11), Over::Tab(TabId(12), false)),
            None
        );
        assert_eq!(
            placement_of_tab(&islands, TabId(11), Over::Tab(TabId(11), false)),
            None,
            "a tab cannot be dropped on itself"
        );
        assert_eq!(
            placement_of_tab(&islands, TabId(10), Over::Island(IslandId(1))),
            None,
            "it already is the first one"
        );
    }

    #[test]
    fn a_tab_crosses_into_another_island() {
        let islands = with_tabs(&[(1, &[10, 11]), (2, &[20])]);
        assert_eq!(
            placement_of_tab(&islands, TabId(10), Over::Tab(TabId(20), true)),
            Some((IslandId(2), 1))
        );
        assert_eq!(
            placement_of_tab(&islands, TabId(10), Over::Island(IslandId(2))),
            Some((IslandId(2), 0)),
            "the header means the top of that island"
        );
        assert_eq!(
            placement_of_tab(&islands, TabId(20), Over::Island(IslandId(1))),
            Some((IslandId(1), 0))
        );
    }

    #[test]
    fn an_empty_island_takes_a_tab_through_its_header() {
        let islands = with_tabs(&[(1, &[10]), (2, &[])]);
        assert_eq!(
            placement_of_tab(&islands, TabId(10), Over::Island(IslandId(2))),
            Some((IslandId(2), 0))
        );
    }

    fn islands(ids: &[i64]) -> Vec<IslandId> {
        ids.iter().map(|id| IslandId(*id)).collect()
    }

    #[test]
    fn an_island_lands_above_or_below_the_one_it_is_dropped_on() {
        let order = islands(&[1, 2, 3, 4]);
        assert_eq!(
            placement(&order, IslandId(1), IslandId(3), false),
            Some((1, 3)),
            "above the third island, which keeps the line"
        );
        assert_eq!(
            placement(&order, IslandId(1), IslandId(3), true),
            Some((2, 4)),
            "below it, so the line belongs to the fourth"
        );
        assert_eq!(
            placement(&order, IslandId(1), IslandId(4), true),
            Some((3, AT_THE_END)),
            "below the last island there is none left to mark"
        );
    }

    #[test]
    fn a_drop_that_changes_nothing_is_no_drop() {
        let order = islands(&[1, 2, 3]);
        assert_eq!(placement(&order, IslandId(2), IslandId(1), true), None);
        assert_eq!(placement(&order, IslandId(2), IslandId(3), false), None);
        assert_eq!(
            placement(&order, IslandId(2), IslandId(2), false),
            None,
            "an island cannot be dropped on itself"
        );
    }

    #[test]
    fn dragging_upwards_counts_without_the_island_itself() {
        let order = islands(&[1, 2, 3, 4]);
        assert_eq!(
            placement(&order, IslandId(4), IslandId(1), false),
            Some((0, 1)),
            "to the very top"
        );
        assert_eq!(
            placement(&order, IslandId(3), IslandId(1), true),
            Some((1, 2))
        );
    }
}
