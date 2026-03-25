use rust_decimal::Decimal;

use crate::types::{OrderId, Quantity};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub order_id: OrderId,
    pub qty: Quantity, // 0 = cancelled or fully filled (tombstone)
}

impl Default for Entry {
    fn default() -> Self {
        Self {
            order_id: 0,
            qty: Decimal::ZERO,
        }
    }
}

const fn is_power_of_two(n: usize) -> bool {
    n > 0 && (n & (n - 1)) == 0
}

/// Fixed-capacity circular buffer for orders at a single price level.
///
/// DEPTH must be a power of two so we can use bitmask indexing instead of
/// modulo. `head` and `tail` are raw monotonic counters; actual array index =
/// `counter as usize & MASK`.
///
/// Fullness is determined by `live_count == DEPTH` — cancelling an entry
/// immediately frees a logical slot without needing to drain tombstones first.
///
/// Head is never advanced eagerly. The matching engine iterates from `head` to
/// `tail` and skips tombstones (qty == 0) inline — no pop/peek needed. Head
/// only advances lazily inside `push` when physical space must be reclaimed.
pub struct RingLevel<const DEPTH: usize> {
    entries: [Entry; DEPTH],
    head: u32,
    tail: u32,
    live_count: u32,
}

const ASSERT_MSG: &str = "DEPTH must be a power of two";

impl<const DEPTH: usize> RingLevel<DEPTH> {
    const MASK: usize = DEPTH - 1;

    pub fn new() -> Self {
        assert!(is_power_of_two(DEPTH), "{}", ASSERT_MSG);
        Self {
            entries: [Entry::default(); DEPTH],
            head: 0,
            tail: 0,
            live_count: 0,
        }
    }

    #[inline]
    pub fn slot(counter: u32) -> usize {
        counter as usize & Self::MASK
    }

    pub fn is_empty(&self) -> bool {
        self.live_count == 0
    }

    pub fn is_full(&self) -> bool {
        self.live_count == DEPTH as u32
    }

    pub fn head(&self) -> u32 {
        self.head
    }

    pub fn tail(&self) -> u32 {
        self.tail
    }

    /// Access the entry at a physical slot.
    #[inline]
    pub fn entry(&self, slot: usize) -> &Entry {
        &self.entries[slot]
    }

    /// Mutable access to the entry at a physical slot.
    #[inline]
    pub fn entry_mut(&mut self, slot: usize) -> &mut Entry {
        &mut self.entries[slot]
    }

    /// Insert an entry at the tail. Returns the slot index, or `None` if full.
    pub fn push(&mut self, order_id: OrderId, qty: Quantity) -> Option<u16> {
        if self.is_full() {
            return None;
        }
        // Reclaim physical space by advancing head past tombstones if needed
        self.reclaim_physical_space();
        let s = Self::slot(self.tail) as u16;
        self.entries[s as usize] = Entry { order_id, qty };
        self.tail = self.tail.wrapping_add(1);
        self.live_count += 1;
        Some(s)
    }

    /// Tombstone an entry by slot index. Sets qty to 0.
    /// Used by both cancel and the matching engine.
    pub fn cancel(&mut self, slot: u16) {
        debug_assert!(self.entries[slot as usize].qty > Decimal::ZERO);
        self.entries[slot as usize].qty = Decimal::ZERO;
        self.live_count -= 1;
    }

    /// When physical span (tail - head) fills the array but live_count says
    /// there's room, advance head past tombstones to free physical slots.
    fn reclaim_physical_space(&mut self) {
        while self.tail.wrapping_sub(self.head) >= DEPTH as u32 {
            self.head = self.head.wrapping_add(1);
        }
    }
}

impl<const DEPTH: usize> Default for RingLevel<DEPTH> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;

    use super::*;

    /// Collect all live entries by scanning head..tail, skipping tombstones.
    fn collect_live<const D: usize>(ring: &RingLevel<D>) -> Vec<(OrderId, Quantity)> {
        let mut out = Vec::new();
        let mut cursor = ring.head();
        while cursor != ring.tail() {
            let e = ring.entry(RingLevel::<D>::slot(cursor));
            if e.qty > Decimal::ZERO {
                out.push((e.order_id, e.qty));
            }
            cursor = cursor.wrapping_add(1);
        }
        out
    }

    #[test]
    fn test_push_and_scan() {
        let mut ring = RingLevel::<4>::new();
        assert!(ring.is_empty());

        ring.push(1, Decimal::from(10)).unwrap();
        ring.push(2, Decimal::from(20)).unwrap();
        assert!(!ring.is_empty());

        let live = collect_live(&ring);
        assert_eq!(live.len(), 2);
        assert_eq!(live[0], (1, Decimal::from(10)));
        assert_eq!(live[1], (2, Decimal::from(20)));
    }

    #[test]
    fn test_cancel_creates_tombstone() {
        let mut ring = RingLevel::<4>::new();
        ring.push(1, Decimal::from(10)).unwrap();
        let slot2 = ring.push(2, Decimal::from(20)).unwrap();
        ring.push(3, Decimal::from(30)).unwrap();

        // Cancel the middle entry
        ring.cancel(slot2);

        // Scan should skip the tombstone
        let live = collect_live(&ring);
        assert_eq!(live.len(), 2);
        assert_eq!(live[0].0, 1);
        assert_eq!(live[1].0, 3);
    }

    #[test]
    fn test_full_rejection() {
        let mut ring = RingLevel::<2>::new();
        assert!(ring.push(1, Decimal::from(10)).is_some());
        assert!(ring.push(2, Decimal::from(20)).is_some());
        assert!(ring.push(3, Decimal::from(30)).is_none());
        assert!(ring.is_full());
    }

    #[test]
    fn test_wrap_around() {
        let mut ring = RingLevel::<4>::new();

        // Fill and cancel twice to force head/tail past DEPTH
        for round in 0..2 {
            let base = round * 4 + 1;
            let s0 = ring.push(base, Decimal::from(10)).unwrap();
            let s1 = ring.push(base + 1, Decimal::from(20)).unwrap();
            let s2 = ring.push(base + 2, Decimal::from(30)).unwrap();
            let s3 = ring.push(base + 3, Decimal::from(40)).unwrap();

            let live = collect_live(&ring);
            assert_eq!(live.len(), 4);
            assert_eq!(live[0].0, base);
            assert_eq!(live[3].0, base + 3);

            ring.cancel(s0);
            ring.cancel(s1);
            ring.cancel(s2);
            ring.cancel(s3);
            assert!(ring.is_empty());
        }
    }

    #[test]
    fn test_scan_skips_tombstones() {
        let mut ring = RingLevel::<4>::new();
        let slot1 = ring.push(1, Decimal::from(10)).unwrap();
        ring.push(2, Decimal::from(20)).unwrap();

        ring.cancel(slot1);

        let live = collect_live(&ring);
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].0, 2);
    }

    #[test]
    fn test_live_count_tracks_correctly() {
        let mut ring = RingLevel::<4>::new();
        assert!(ring.is_empty());

        let s1 = ring.push(1, Decimal::from(10)).unwrap();
        let s2 = ring.push(2, Decimal::from(20)).unwrap();
        assert!(!ring.is_empty());

        ring.cancel(s1);
        assert!(!ring.is_empty()); // still has order 2

        ring.cancel(s2);
        assert!(ring.is_empty());
    }

    #[test]
    fn test_cancel_all_then_push() {
        let mut ring = RingLevel::<4>::new();
        let s1 = ring.push(1, Decimal::from(10)).unwrap();
        let s2 = ring.push(2, Decimal::from(20)).unwrap();
        let s3 = ring.push(3, Decimal::from(30)).unwrap();

        ring.cancel(s1);
        ring.cancel(s2);
        ring.cancel(s3);
        assert!(ring.is_empty());
        assert!(!ring.is_full());

        // Push reclaims physical space from tombstones automatically
        ring.push(4, Decimal::from(40)).unwrap();
        let live = collect_live(&ring);
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].0, 4);
    }
}
