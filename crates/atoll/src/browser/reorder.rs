//! Dragging an island to another place in the sidebar.
//!
//! The row that is being dragged keeps the pointer, so the other rows never hear about the
//! pointer themselves. They are told where it is instead and report when it stands over them,
//! which lands here as a target and a side. This turns that into the place the island would get
//! and shows a line there, so the drop holds no surprise.

use atoll_core::IslandId;
use slint::ComponentHandle;

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
