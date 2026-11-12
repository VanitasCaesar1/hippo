//! The counts the sidebar's nav shows (the sidebar, `sidebar.rs`).

use super::Shell;

/// Below this window width the sidebar does not dock (an overlay).
pub const NARROW: f32 = super::sidebar::NARROW;

impl Shell {
    /// The changes waiting in the Inbox.
    pub(crate) fn inbox_count(&self) -> usize {
        self.review_items().len() + self.lessons().len()
    }
}
