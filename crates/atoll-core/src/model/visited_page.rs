use crate::time::Timestamp;

/// An entry of the global browsing history, one per address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisitedPage {
    pub url: String,
    pub title: Option<String>,
    pub visit_count: i64,
    pub last_visited_at: Timestamp,
}
