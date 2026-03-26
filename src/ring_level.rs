use rust_decimal::Decimal;

use crate::types::{OrderId, Quantity};

const fn is_power_of_two(n: usize) -> bool {
    n > 0 && (n & (n - 1)) == 0
}

/// Fixed-capacity append-only buffer for orders at a single price level.
///
/// Uses a Struct-of-Arrays (SoA) layout: `qtys` and `order_ids` are stored in
/// separate contiguous arrays. A bitmap tracks live/dead status so the scan
/// path only touches ~128 bytes (for DEPTH=1024) instead of the 16KB qty array.
///
/// DEPTH must be a power of two for bitmask indexing (`slot = counter & MASK`).
/// `tail` advances on each `push`; the buffer is full when `tail - head == DEPTH`.
/// Cancelling clears a bitmap bit but does not free the physical slot.
/// Use `compact_level` on the parent book to reclaim tombstoned slots.
pub struct RingLevel<const DEPTH: usize> {
    // Warm: read only when bitmap says entry is live
    qtys: [Quantity; DEPTH],
    order_ids: [OrderId; DEPTH],
    // Hot: scanned to skip tombstones — one bit per slot.
    // For DEPTH=1024 this is 16 u64s = 128 bytes = 2 cache lines.
    bitmap: Box<[u64]>,
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
            qtys: [Decimal::ZERO; DEPTH],
            order_ids: [0; DEPTH],
            bitmap: vec![0u64; DEPTH.div_ceil(64)].into_boxed_slice(),
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

    #[inline]
    pub fn order_id(&self, slot: usize) -> OrderId {
        self.order_ids[slot]
    }

    #[inline]
    pub fn qty(&self, slot: usize) -> Quantity {
        self.qtys[slot]
    }

    /// Check whether the entry at `slot` is live (not cancelled).
    /// Uses the bitmap — a single bit test on a small, cache-resident array.
    #[inline]
    pub fn is_live(&self, slot: usize) -> bool {
        self.bitmap[slot >> 6] & (1u64 << (slot & 63)) != 0
    }

    #[inline]
    fn bitmap_set(&mut self, slot: usize) {
        self.bitmap[slot >> 6] |= 1u64 << (slot & 63);
    }

    #[inline]
    fn bitmap_clear(&mut self, slot: usize) {
        self.bitmap[slot >> 6] &= !(1u64 << (slot & 63));
    }

    /// Insert an entry at `tail`. Returns the slot index, or `None` if full.
    pub fn push(&mut self, order_id: OrderId, qty: Quantity) -> Option<u16> {
        if self.is_full() {
            return None;
        }
        let s = Self::slot(self.tail) as u16;
        self.order_ids[s as usize] = order_id;
        self.qtys[s as usize] = qty;
        self.bitmap_set(s as usize);
        self.tail += 1;
        self.live_count += 1;
        Some(s)
    }

    #[inline]
    pub fn set_qty(&mut self, slot: usize, qty: Quantity) {
        self.qtys[slot] = qty;
    }

    /// Tombstone an entry by slot index. Clears the bitmap bit.
    /// The qty value is left as-is — the bitmap is the source of truth.
    pub fn cancel(&mut self, slot: u16) {
        debug_assert!(self.is_live(slot as usize));
        self.bitmap_clear(slot as usize);
        self.live_count -= 1;
    }

    /// Reset the ring to empty state. Used during compaction.
    pub fn reset(&mut self) {
        for word in self.bitmap.iter_mut() {
            *word = 0;
        }
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
            let slot = RingLevel::<D>::slot(cursor);
            if ring.is_live(slot) {
                out.push((ring.order_id(slot), ring.qty(slot)));
            }
            cursor += 1;
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

        ring.cancel(slot2);

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
    fn test_cancel_does_not_free_physical_capacity() {
        let mut ring = RingLevel::<4>::new();

        ring.push(1, Decimal::from(10)).unwrap();
        let s1 = ring.push(2, Decimal::from(20)).unwrap();
        ring.push(3, Decimal::from(30)).unwrap();
        ring.push(4, Decimal::from(40)).unwrap();
        assert!(ring.is_full());

        ring.cancel(s1);
        assert!(ring.is_full());

        assert!(ring.push(5, Decimal::from(50)).is_none());
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
        assert!(!ring.is_empty());

        ring.cancel(s2);
        assert!(ring.is_empty());
    }

    #[test]
    fn test_cancel_tombstones_but_scan_skips() {
        let mut ring = RingLevel::<4>::new();
        ring.push(1, Decimal::from(10)).unwrap();
        let s2 = ring.push(2, Decimal::from(20)).unwrap();
        ring.push(3, Decimal::from(30)).unwrap();
        ring.push(4, Decimal::from(40)).unwrap();

        ring.cancel(s2);

        let live = collect_live(&ring);
        assert_eq!(live.len(), 3);
        assert_eq!(live[0].0, 1);
        assert_eq!(live[1].0, 3);
        assert_eq!(live[2].0, 4);
    }

    #[test]
    fn test_bitmap_word_boundary() {
        // DEPTH=128 uses 2 bitmap words — test entries spanning the boundary
        let mut ring = RingLevel::<128>::new();
        // Push entries at slots 62, 63, 64, 65 (straddles word 0/1 boundary)
        for i in 0..66 {
            ring.push(i + 1, Decimal::from(i as i64 + 1)).unwrap();
        }
        // Cancel slots 63 and 64 (last bit of word 0, first bit of word 1)
        ring.cancel(63);
        ring.cancel(64);

        assert!(!ring.is_live(63));
        assert!(!ring.is_live(64));
        assert!(ring.is_live(62));
        assert!(ring.is_live(65));

        let live = collect_live(&ring);
        assert_eq!(live.len(), 64); // 66 - 2 cancelled
    }
}
