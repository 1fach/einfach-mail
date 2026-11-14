/// The number of unread messages in a folder.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct UnreadCount(u32);

impl UnreadCount {
    /// Creates a count from a number of unread messages.
    #[must_use]
    pub const fn new(count: u32) -> Self {
        Self(count)
    }

    /// Returns the number of unread messages.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}
