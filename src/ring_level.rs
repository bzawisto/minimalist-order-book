use rust_decimal::Decimal;

use crate::types::{OrderId, Quantity};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(64))]
pub struct Entry {
    pub order_id: OrderId,
    pub qty: Quantity, // 0 = cancelled or fully filled (tombstone)
    pub timestamp: u64,
}

impl Default for Entry {
    fn default() -> Self {
        Self {
            order_id: 0,
            qty: Decimal::ZERO,
            timestamp: 0,
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
/// Fullness is determined by physical saturation: `tail - head == DEPTH`.
/// Cancelling an entry tombstones it but does not free physical capacity —
/// only consuming entries from the front (advancing `head`) does.
/// DEPTH should be sized so the level never overflows.
///
/// Head never moves. Tail only advances on `push`. The matching engine
/// iterates from `head` to `tail` and skips tombstones (qty == 0) inline.
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
        (self.tail - self.head) as usize == DEPTH
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

    /// Insert an entry at `tail`. Returns the slot index, or `None` if full.
    ///
    /// Full means `tail - head == DEPTH` — the physical ring is saturated.
    /// Cancel does not free physical slots; only consuming from the front
    /// (advancing `head`) does. New entries always go at `tail & MASK`,
    /// preserving FIFO order.
    pub fn push(&mut self, order_id: OrderId, qty: Quantity, timestamp: u64) -> Option<u16> {
        if self.is_full() {
            return None;
        }
        let s = Self::slot(self.tail) as u16;
        self.entries[s as usize] = Entry {
            order_id,
            qty,
            timestamp,
        };
        self.tail += 1;
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

    /// Reset the ring to empty state. Used during compaction.
    pub fn reset(&mut self) {
        self.head = 0;
        self.tail = 0;
        self.live_count = 0;
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
            cursor += 1;
        }
        out
    }

    #[test]
    fn test_push_and_scan() {
        let mut ring = RingLevel::<4>::new();
        assert!(ring.is_empty());

        ring.push(1, Decimal::from(10), 0).unwrap();
        ring.push(2, Decimal::from(20), 0).unwrap();
        assert!(!ring.is_empty());

        let live = collect_live(&ring);
        assert_eq!(live.len(), 2);
        assert_eq!(live[0], (1, Decimal::from(10)));
        assert_eq!(live[1], (2, Decimal::from(20)));
    }

    #[test]
    fn test_cancel_creates_tombstone() {
        let mut ring = RingLevel::<4>::new();
        ring.push(1, Decimal::from(10), 0).unwrap();
        let slot2 = ring.push(2, Decimal::from(20), 0).unwrap();
        ring.push(3, Decimal::from(30), 0).unwrap();

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
        assert!(ring.push(1, Decimal::from(10), 0).is_some());
        assert!(ring.push(2, Decimal::from(20), 0).is_some());
        assert!(ring.push(3, Decimal::from(30), 0).is_none());
        assert!(ring.is_full());
    }

    #[test]
    fn test_cancel_does_not_free_physical_capacity() {
        let mut ring = RingLevel::<4>::new();

        // Push 4 entries to fill the ring
        ring.push(1, Decimal::from(10), 0).unwrap();
        let s1 = ring.push(2, Decimal::from(20), 0).unwrap();
        ring.push(3, Decimal::from(30), 0).unwrap();
        ring.push(4, Decimal::from(40), 0).unwrap();
        assert!(ring.is_full());

        // Cancel doesn't free physical capacity — still full
        ring.cancel(s1);
        assert!(ring.is_full());

        // Can't push even though there's a tombstone
        assert!(ring.push(5, Decimal::from(50), 0).is_none());
    }

    #[test]
    fn test_scan_skips_tombstones() {
        let mut ring = RingLevel::<4>::new();
        let slot1 = ring.push(1, Decimal::from(10), 0).unwrap();
        ring.push(2, Decimal::from(20), 0).unwrap();

        ring.cancel(slot1);

        let live = collect_live(&ring);
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].0, 2);
    }

    #[test]
    fn test_live_count_tracks_correctly() {
        let mut ring = RingLevel::<4>::new();
        assert!(ring.is_empty());

        let s1 = ring.push(1, Decimal::from(10), 0).unwrap();
        let s2 = ring.push(2, Decimal::from(20), 0).unwrap();
        assert!(!ring.is_empty());

        ring.cancel(s1);
        assert!(!ring.is_empty()); // still has order 2

        ring.cancel(s2);
        assert!(ring.is_empty());
    }

    #[test]
    fn test_cancel_tombstones_but_scan_skips() {
        let mut ring = RingLevel::<4>::new();
        ring.push(1, Decimal::from(10), 0).unwrap();
        let s2 = ring.push(2, Decimal::from(20), 0).unwrap();
        ring.push(3, Decimal::from(30), 0).unwrap();
        ring.push(4, Decimal::from(40), 0).unwrap();

        ring.cancel(s2);

        // Scan still sees the 3 live entries, skipping the tombstone
        let live = collect_live(&ring);
        assert_eq!(live.len(), 3);
        assert_eq!(live[0].0, 1);
        assert_eq!(live[1].0, 3);
        assert_eq!(live[2].0, 4);
    }
}
