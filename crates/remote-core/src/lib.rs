//! Terminal ownership and the lifecycle of the development device's tunnel agent.
pub mod terminal;
pub mod tunnel_agent;
use std::sync::atomic::{AtomicU64, Ordering};

/// Identifies one terminal instance, even when two tabs connect to the same device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(u64);

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl SessionId {
    pub fn allocate() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// Only inactive tabs live here. The adapter owns the active tab.
pub struct ParkedTabs<T>(Vec<(SessionId, T)>);

impl<T> Default for ParkedTabs<T> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<T> ParkedTabs<T> {
    /// Park an active tab. Its ID must not already be parked.
    pub fn park(&mut self, id: SessionId, tab: T) {
        debug_assert!(!self.0.iter().any(|(key, _)| *key == id));
        self.0.push((id, tab));
    }

    pub fn take_recent(&mut self) -> Option<T> {
        self.0.pop().map(|(_, tab)| tab)
    }

    /// Cycle in creation order, wrapping from the newest to the oldest tab.
    pub fn take_next(&mut self, after: SessionId) -> Option<T> {
        let id = self
            .0
            .iter()
            .map(|(id, _)| *id)
            .min_by_key(|id| (*id <= after, *id))?;
        self.remove(id)
    }

    pub fn remove(&mut self, id: SessionId) -> Option<T> {
        let index = self.0.iter().position(|(key, _)| *key == id)?;
        Some(self.0.remove(index).1)
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = (SessionId, &T)> {
        self.0.iter().map(|(id, tab)| (*id, tab))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycling_closing_and_resuming_use_instance_identity() {
        let [a, b, c] = std::array::from_fn(|_| SessionId::allocate());
        let mut tabs = ParkedTabs::default();
        tabs.park(b, "b");
        tabs.park(a, "a");
        assert_eq!(tabs.take_next(c), Some("a"));
        tabs.park(c, "c");
        assert_eq!(tabs.take_next(a), Some("b"));
        tabs.park(a, "a");
        assert_eq!(tabs.remove(b), None); // late event for a previously closed tab
        assert_eq!(tabs.take_recent(), Some("a"));
        assert_eq!(tabs.remove(c), Some("c"));
        assert_eq!(tabs.take_recent(), None);
    }
}
