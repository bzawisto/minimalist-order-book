use rust_decimal::{Decimal, prelude::ToPrimitive};
use rustc_hash::FxHashMap;
use thiserror::Error;

use crate::{
    ring_level::RingLevel,
    types::{OrderId, Price, Quantity, Side},
};

#[derive(Error, Debug)]
pub enum FixedBookError {
    #[error("Price {0} is outside the book range [{1}, {2}]")]
    PriceOutOfRange(Price, Price, Price),
    #[error("Level at price {0} is full (max {1} orders)")]
    LevelFull(Price, usize),
    #[error("Order ID {0} not found")]
    NotFound(OrderId),
}

pub struct FixedBookSide<const LEVELS: usize, const DEPTH: usize> {
    levels: Box<[RingLevel<DEPTH>; LEVELS]>,
    side: Side,
    best_level: Option<usize>,
}

impl<const LEVELS: usize, const DEPTH: usize> FixedBookSide<LEVELS, DEPTH> {
    pub fn new(side: Side) -> Self {
        Self {
            levels: Self::alloc_levels(),
            side,
            best_level: None,
        }
    }

    fn alloc_levels() -> Box<[RingLevel<DEPTH>; LEVELS]> {
        let v: Vec<RingLevel<DEPTH>> = (0..LEVELS).map(|_| RingLevel::new()).collect();
        v.into_boxed_slice()
            .try_into()
            .unwrap_or_else(|_| unreachable!())
    }

    /// Push an order into the level at `level_idx`. Returns the slot index.
    pub fn push_at(&mut self, level_idx: usize, order_id: OrderId, qty: Quantity) -> Option<u16> {
        let slot = self.levels[level_idx].push(order_id, qty)?;
        self.update_best_after_insert(level_idx);
        Some(slot)
    }

    /// Cancel (tombstone) an order at the given level and slot.
    pub fn cancel_at(&mut self, level_idx: usize, slot: u16) {
        self.levels[level_idx].cancel(slot);
        if self.levels[level_idx].is_empty() && self.best_level == Some(level_idx) {
            self.best_level = self.scan_for_best();
        }
    }

    pub fn best_level_idx(&self) -> Option<usize> {
        self.best_level
    }

    fn update_best_after_insert(&mut self, level_idx: usize) {
        match self.best_level {
            None => self.best_level = Some(level_idx),
            Some(current) => {
                let is_better = match self.side {
                    Side::Buy => level_idx > current,  // higher = better for bids
                    Side::Sell => level_idx < current, // lower = better for asks
                };
                if is_better {
                    self.best_level = Some(level_idx);
                }
            }
        }
    }

    fn scan_for_best(&self) -> Option<usize> {
        match self.side {
            // Bids: scan from highest index downward
            Side::Buy => (0..LEVELS).rev().find(|&i| !self.levels[i].is_empty()),
            // Asks: scan from lowest index upward
            Side::Sell => (0..LEVELS).find(|&i| !self.levels[i].is_empty()),
        }
    }
}

pub struct FixedOrderBook<const LEVELS: usize, const DEPTH: usize> {
    bids: FixedBookSide<LEVELS, DEPTH>,
    asks: FixedBookSide<LEVELS, DEPTH>,
    /// Maps order_id → (side, level_index, slot_in_ring)
    order_index: FxHashMap<OrderId, (Side, usize, u16)>,
    /// Scratch buffer used by `compact_level` to avoid aliasing during compaction.
    scratch: Box<RingLevel<DEPTH>>,
    base_price: Price,
    tick_size: Price,
}

impl<const LEVELS: usize, const DEPTH: usize> FixedOrderBook<LEVELS, DEPTH> {
    pub fn new(base_price: Price, tick_size: Price) -> Self {
        Self {
            bids: FixedBookSide::new(Side::Buy),
            asks: FixedBookSide::new(Side::Sell),
            order_index: FxHashMap::default(),
            scratch: Box::new(RingLevel::new()),
            base_price,
            tick_size,
        }
    }

    pub fn add_limit_order(
        &mut self,
        id: OrderId,
        side: Side,
        price: Price,
        qty: Quantity,
    ) -> Result<OrderId, FixedBookError> {
        let level_idx = self.price_to_index(price)?;

        let book_side = match side {
            Side::Buy => &mut self.bids,
            Side::Sell => &mut self.asks,
        };

        let slot = book_side
            .push_at(level_idx, id, qty)
            .ok_or(FixedBookError::LevelFull(price, DEPTH))?;

        self.order_index.insert(id, (side, level_idx, slot));

        Ok(id)
    }

    /// Compact a level by removing tombstones.
    ///
    /// Copies live entries into the scratch buffer, swaps it with the target
    /// level, then updates the order_index with new slot positions.
    pub fn compact_level(&mut self, side: Side, level_idx: usize) {
        let Self {
            bids,
            asks,
            scratch,
            order_index,
            ..
        } = self;

        let level = match side {
            Side::Buy => &mut bids.levels[level_idx],
            Side::Sell => &mut asks.levels[level_idx],
        };

        // Copy live entries into scratch
        scratch.reset();
        let mut cursor = level.head();
        while cursor != level.tail() {
            let slot = RingLevel::<DEPTH>::slot(cursor);
            if level.is_live(slot) {
                scratch.push(level.order_id(slot), level.qty(slot));
            }
            cursor += 1;
        }

        // Swap scratch and level — level now has the compacted entries
        std::mem::swap(level, scratch);

        // Update order_index with new slot positions
        let mut cursor = level.head();
        while cursor != level.tail() {
            let slot = RingLevel::<DEPTH>::slot(cursor) as u16;
            let oid = level.order_id(slot as usize);
            if let Some((_, _, stored_slot)) = order_index.get_mut(&oid) {
                *stored_slot = slot;
            }
            cursor += 1;
        }
    }

    pub fn cancel_order(&mut self, id: OrderId) -> Result<(), FixedBookError> {
        let (side, level_idx, slot) = self
            .order_index
            .remove(&id)
            .ok_or(FixedBookError::NotFound(id))?;

        match side {
            Side::Buy => self.bids.cancel_at(level_idx, slot),
            Side::Sell => self.asks.cancel_at(level_idx, slot),
        }

        Ok(())
    }

    pub fn best_bid(&self) -> Option<Price> {
        self.bids
            .best_level_idx()
            .map(|idx| self.index_to_price(idx))
    }

    pub fn best_ask(&self) -> Option<Price> {
        self.asks
            .best_level_idx()
            .map(|idx| self.index_to_price(idx))
    }

    pub fn spread(&self) -> Option<(Price, Price)> {
        let bid = self.best_bid()?;
        let ask = self.best_ask()?;
        Some((bid, ask))
    }

    fn price_to_index(&self, price: Price) -> Result<usize, FixedBookError> {
        let offset = price - self.base_price;
        if offset.is_sign_negative() {
            return Err(self.range_error(price));
        }
        let ticks = offset / self.tick_size;
        // Reject non-tick-aligned prices
        if ticks.fract() != Decimal::ZERO {
            return Err(self.range_error(price));
        }
        let idx = ticks.to_usize().ok_or_else(|| self.range_error(price))?;
        if idx >= LEVELS {
            return Err(self.range_error(price));
        }
        Ok(idx)
    }

    fn index_to_price(&self, idx: usize) -> Price {
        self.base_price + self.tick_size * Decimal::from(idx)
    }

    fn range_error(&self, price: Price) -> FixedBookError {
        let max_price = self.base_price + self.tick_size * Decimal::from(LEVELS - 1);
        FixedBookError::PriceOutOfRange(price, self.base_price, max_price)
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;

    use super::*;

    /// Book with base=90, tick=1, 30 levels → prices 90..=119
    fn make_book() -> FixedOrderBook<30, 64> {
        FixedOrderBook::<30, 64>::new(Decimal::from(90), Decimal::from(1))
    }

    #[test]
    fn test_add_and_cancel_order() {
        let mut book = make_book();
        let id1 = book
            .add_limit_order(1, Side::Buy, Decimal::from(100), Decimal::from(10))
            .unwrap();
        let id2 = book
            .add_limit_order(2, Side::Sell, Decimal::from(105), Decimal::from(10))
            .unwrap();

        assert_eq!(book.best_bid(), Some(Decimal::from(100)));
        assert_eq!(book.best_ask(), Some(Decimal::from(105)));
        assert_eq!(
            book.spread(),
            Some((Decimal::from(100), Decimal::from(105)))
        );

        assert!(book.cancel_order(id1).is_ok());
        assert_eq!(book.best_bid(), None);
        assert_eq!(book.spread(), None);

        assert!(book.cancel_order(id2).is_ok());
        assert_eq!(book.best_ask(), None);
    }

    #[test]
    fn test_cancel_nonexistent_order() {
        let mut book = make_book();
        assert!(book.cancel_order(999).is_err());
    }

    #[test]
    fn test_best_bid_ask_ordering() {
        let mut book = make_book();
        book.add_limit_order(1, Side::Buy, Decimal::from(100), Decimal::from(10))
            .unwrap();
        book.add_limit_order(2, Side::Buy, Decimal::from(110), Decimal::from(10))
            .unwrap();
        book.add_limit_order(3, Side::Buy, Decimal::from(105), Decimal::from(10))
            .unwrap();
        assert_eq!(book.best_bid(), Some(Decimal::from(110)));

        book.add_limit_order(4, Side::Sell, Decimal::from(115), Decimal::from(10))
            .unwrap();
        book.add_limit_order(5, Side::Sell, Decimal::from(112), Decimal::from(10))
            .unwrap();
        book.add_limit_order(6, Side::Sell, Decimal::from(119), Decimal::from(10))
            .unwrap();
        assert_eq!(book.best_ask(), Some(Decimal::from(112)));
    }

    #[test]
    fn test_spread() {
        let mut book = make_book();
        assert_eq!(book.spread(), None);

        book.add_limit_order(1, Side::Buy, Decimal::from(100), Decimal::from(10))
            .unwrap();
        assert_eq!(book.spread(), None);

        book.add_limit_order(2, Side::Sell, Decimal::from(105), Decimal::from(10))
            .unwrap();
        assert_eq!(
            book.spread(),
            Some((Decimal::from(100), Decimal::from(105)))
        );

        // Narrow the spread
        book.add_limit_order(3, Side::Buy, Decimal::from(102), Decimal::from(10))
            .unwrap();
        book.add_limit_order(4, Side::Sell, Decimal::from(104), Decimal::from(10))
            .unwrap();
        assert_eq!(
            book.spread(),
            Some((Decimal::from(102), Decimal::from(104)))
        );

        // Remove inner orders, spread should widen
        let id5 = book
            .add_limit_order(5, Side::Buy, Decimal::from(103), Decimal::from(10))
            .unwrap();
        assert_eq!(
            book.spread(),
            Some((Decimal::from(103), Decimal::from(104)))
        );
        book.cancel_order(id5).unwrap();
        assert_eq!(
            book.spread(),
            Some((Decimal::from(102), Decimal::from(104)))
        );
    }

    #[test]
    fn test_price_out_of_range() {
        let mut book = make_book(); // prices 90..=119
        // Below base_price
        assert!(
            book.add_limit_order(1, Side::Buy, Decimal::from(89), Decimal::from(10))
                .is_err()
        );
        // Above max
        assert!(
            book.add_limit_order(2, Side::Buy, Decimal::from(120), Decimal::from(10))
                .is_err()
        );
        // At boundaries: OK
        assert!(
            book.add_limit_order(3, Side::Buy, Decimal::from(90), Decimal::from(10))
                .is_ok()
        );
        assert!(
            book.add_limit_order(4, Side::Sell, Decimal::from(119), Decimal::from(10))
                .is_ok()
        );
    }

    #[test]
    fn test_level_full() {
        let mut book = FixedOrderBook::<10, 2>::new(Decimal::from(100), Decimal::from(1));
        book.add_limit_order(1, Side::Buy, Decimal::from(105), Decimal::from(10))
            .unwrap();
        book.add_limit_order(2, Side::Buy, Decimal::from(105), Decimal::from(20))
            .unwrap();
        // Third order at same price should fail (DEPTH=2)
        let result = book.add_limit_order(3, Side::Buy, Decimal::from(105), Decimal::from(30));
        assert!(result.is_err());
    }

    #[test]
    fn test_cancel_best_bid_updates_best() {
        let mut book = make_book();
        let id_110 = book
            .add_limit_order(1, Side::Buy, Decimal::from(110), Decimal::from(10))
            .unwrap();
        book.add_limit_order(2, Side::Buy, Decimal::from(100), Decimal::from(10))
            .unwrap();

        assert_eq!(book.best_bid(), Some(Decimal::from(110)));
        book.cancel_order(id_110).unwrap();
        assert_eq!(book.best_bid(), Some(Decimal::from(100)));
    }

    #[test]
    fn test_cancel_best_ask_updates_best() {
        let mut book = make_book();
        let id_95 = book
            .add_limit_order(1, Side::Sell, Decimal::from(95), Decimal::from(10))
            .unwrap();
        book.add_limit_order(2, Side::Sell, Decimal::from(105), Decimal::from(10))
            .unwrap();

        assert_eq!(book.best_ask(), Some(Decimal::from(95)));
        book.cancel_order(id_95).unwrap();
        assert_eq!(book.best_ask(), Some(Decimal::from(105)));
    }

    #[test]
    fn test_multiple_orders_same_level_cancel_preserves_best() {
        let mut book = make_book();
        let id1 = book
            .add_limit_order(1, Side::Buy, Decimal::from(100), Decimal::from(10))
            .unwrap();
        book.add_limit_order(2, Side::Buy, Decimal::from(100), Decimal::from(20))
            .unwrap();

        // Cancel one order at the best level, but the level still has orders
        book.cancel_order(id1).unwrap();
        assert_eq!(book.best_bid(), Some(Decimal::from(100)));
    }

    #[test]
    fn test_compact_level() {
        let mut book = FixedOrderBook::<10, 8>::new(Decimal::from(100), Decimal::from(1));
        // Add 4 orders at price 105, cancel 2 in the middle
        let id1 = book
            .add_limit_order(1, Side::Buy, Decimal::from(105), Decimal::from(10))
            .unwrap();
        let _id2 = book
            .add_limit_order(2, Side::Buy, Decimal::from(105), Decimal::from(20))
            .unwrap();
        let id3 = book
            .add_limit_order(3, Side::Buy, Decimal::from(105), Decimal::from(30))
            .unwrap();
        let _id4 = book
            .add_limit_order(4, Side::Buy, Decimal::from(105), Decimal::from(40))
            .unwrap();

        book.cancel_order(id1).unwrap();
        book.cancel_order(id3).unwrap();

        // Level has 4 physical entries (2 tombstones, 2 live)
        let level_idx = 5; // price 105 - base 100 = 5
        book.compact_level(Side::Buy, level_idx);

        // After compaction: head=0, tail=2, 2 live entries, no tombstones
        // Orders should still be cancellable via order_index
        assert_eq!(book.best_bid(), Some(Decimal::from(105)));
        book.cancel_order(_id2).unwrap();
        book.cancel_order(_id4).unwrap();
        assert_eq!(book.best_bid(), None);
    }

    #[test]
    fn test_non_tick_aligned_price_rejected() {
        let mut book = FixedOrderBook::<100, 64>::new(Decimal::from(100), Decimal::new(1, 2)); // tick = 0.01
        // 100.005 is not aligned to 0.01
        assert!(
            book.add_limit_order(1, Side::Buy, Decimal::new(100005, 3), Decimal::from(10))
                .is_err()
        );
        // 100.01 is aligned
        assert!(
            book.add_limit_order(2, Side::Buy, Decimal::new(10001, 2), Decimal::from(10))
                .is_ok()
        );
    }
}
