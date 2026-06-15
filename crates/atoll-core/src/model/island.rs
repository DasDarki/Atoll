use crate::ids::{IslandId, WorkspaceId};
use crate::time::Timestamp;

/// A named, collapsible group of tabs inside a workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Island {
    pub id: IslandId,
    pub workspace_id: WorkspaceId,
    pub name: String,
    pub position: i64,
    pub collapsed: bool,
    pub color: IslandColor,
    pub archived_at: Option<Timestamp>,
    pub last_used_at: Option<Timestamp>,
}

impl Island {
    pub fn is_archived(&self) -> bool {
        self.archived_at.is_some()
    }
}

/// The accent colour of an island, persisted as its index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum IslandColor {
    #[default]
    Violet,
    Blue,
    Cyan,
    Teal,
    Green,
    Amber,
    Rose,
    Slate,
}

impl IslandColor {
    pub const ALL: [IslandColor; 8] = [
        IslandColor::Violet,
        IslandColor::Blue,
        IslandColor::Cyan,
        IslandColor::Teal,
        IslandColor::Green,
        IslandColor::Amber,
        IslandColor::Rose,
        IslandColor::Slate,
    ];

    pub fn index(self) -> i64 {
        Self::ALL
            .iter()
            .position(|color| *color == self)
            .unwrap_or_default() as i64
    }

    pub fn from_index(index: i64) -> Self {
        usize::try_from(index)
            .ok()
            .and_then(|index| Self::ALL.get(index).copied())
            .unwrap_or_default()
    }

    /// Picks a colour for a new island so that neighbouring islands differ.
    pub fn for_position(position: usize) -> Self {
        Self::ALL[position % Self::ALL.len()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_index_round_trips() {
        for color in IslandColor::ALL {
            assert_eq!(IslandColor::from_index(color.index()), color);
        }
    }

    #[test]
    fn unknown_color_index_falls_back_to_default() {
        assert_eq!(IslandColor::from_index(-1), IslandColor::Violet);
        assert_eq!(IslandColor::from_index(99), IslandColor::Violet);
    }
}
