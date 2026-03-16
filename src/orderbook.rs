use std::{
    cmp::Reverse,
    collections::{BTreeMap, HashMap},
};

use thiserror::Error;

use crate::{
    order::Order,
    types::{OrderId, Price, Quantity, Side},
};

#[derive(Error, Debug)]
pub enum OrderError {
    #[error("Order ID {0} not found")]
    NotFound(OrderId),
    #[error("Wait")]
    Other,
}

pub struct OrderBook {
    // Highest price first for bids
    pub bids: BTreeMap<Reverse<Price>, Vec<Order>>,
    // Lowest price first for asks
    pub asks: BTreeMap<Price, Vec<Order>>,
    // Index to quickly find an order by its ID
    pub order_index: HashMap<OrderId, Price>,
    next_order_id: OrderId,
}

impl OrderBook {
    pub fn new() -> Self {
        Self {
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            order_index: HashMap::new(),
            next_order_id: 1,
        }
    }

    pub fn add_limit_order(
        &mut self,
        side: Side,
        price: Price,
        qty: Quantity,
        timestamp: u64,
    ) -> OrderId {
        let order = Order {
            id: self.next_order_id,
            side,
            price,
            quantity: qty,
            remaining: qty,
            timestamp,
        };
        self.next_order_id += 1;

        let order_id = order.id;
        self.order_index.insert(order_id, price);

        match side {
            Side::Buy => {
                self.bids.entry(Reverse(price)).or_default().push(order);
            }
            Side::Sell => {
                self.asks.entry(price).or_default().push(order);
            }
        }

        order_id
    }

    pub fn cancel_order(&mut self, id: OrderId) -> Result<(), OrderError> {
        let price = self
            .order_index
            .remove(&id)
            .ok_or(OrderError::NotFound(id))?;

        if let Some(list) = self.bids.get_mut(&Reverse(price)) {
            if let Some(pos) = list.iter().position(|o| o.id == id) {
                // Map the trailing region directly via slice::copy_within which is just ptr::copy memory shift
                list.copy_within((pos + 1).., pos);
                list.pop();

                if list.is_empty() {
                    self.bids.remove(&Reverse(price));
                }
                return Ok(());
            }
        }

        if let Some(list) = self.asks.get_mut(&price) {
            if let Some(pos) = list.iter().position(|o| o.id == id) {
                // Map the trailing region directly via slice::copy_within which is just ptr::copy memory shift
                list.copy_within((pos + 1).., pos);
                list.pop();

                if list.is_empty() {
                    self.asks.remove(&price);
                }
                return Ok(());
            }
        }

        // We shouldn't hit this if the index and orderbook are perfectly synchronized
        Err(OrderError::NotFound(id))
    }

    pub fn best_bid(&self) -> Option<Price> {
        // The first element in bids is the highest price, because it's a Reverse<Price>
        self.bids
            .iter()
            .filter(|(_, q)| !q.is_empty())
            .map(|(rev, _)| rev.0)
            .next()
    }

    pub fn best_ask(&self) -> Option<Price> {
        self.asks
            .iter()
            .filter(|(_, q)| !q.is_empty())
            .map(|(&p, _)| p)
            .next()
    }

    pub fn spread(&self) -> Option<(Price, Price)> {
        let bid = self.best_bid()?;
        let ask = self.best_ask()?;
        Some((bid, ask))
    }
}

impl Default for OrderBook {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;

    use super::*;

    #[test]
    fn test_add_and_cancel_order() {
        let mut order_book = OrderBook::new();
        let id1 = order_book.add_limit_order(Side::Buy, Decimal::from(100), Decimal::from(10), 0);
        let id2 = order_book.add_limit_order(Side::Sell, Decimal::from(105), Decimal::from(10), 1);

        assert_eq!(order_book.best_bid(), Some(Decimal::from(100)));
        assert_eq!(order_book.best_ask(), Some(Decimal::from(105)));
        assert_eq!(
            order_book.spread(),
            Some((Decimal::from(100), Decimal::from(105)))
        );

        assert!(order_book.cancel_order(id1).is_ok());
        assert_eq!(order_book.best_bid(), None);
        assert_eq!(order_book.spread(), None);

        assert!(order_book.cancel_order(id2).is_ok());
        assert_eq!(order_book.best_ask(), None);
    }

    #[test]
    fn test_cancel_nonexistent_order() {
        let mut order_book = OrderBook::new();
        assert!(order_book.cancel_order(999).is_err());
    }

    #[test]
    fn test_best_bid_ask_ordering() {
        let mut order_book = OrderBook::new();
        order_book.add_limit_order(Side::Buy, Decimal::from(100), Decimal::from(10), 0);
        order_book.add_limit_order(Side::Buy, Decimal::from(110), Decimal::from(10), 1);
        order_book.add_limit_order(Side::Buy, Decimal::from(105), Decimal::from(10), 2);
        assert_eq!(order_book.best_bid(), Some(Decimal::from(110)));

        order_book.add_limit_order(Side::Sell, Decimal::from(120), Decimal::from(10), 3);
        order_book.add_limit_order(Side::Sell, Decimal::from(115), Decimal::from(10), 4);
        order_book.add_limit_order(Side::Sell, Decimal::from(125), Decimal::from(10), 5);
        assert_eq!(order_book.best_ask(), Some(Decimal::from(115)));
    }

    #[test]
    fn test_spread() {
        let mut order_book = OrderBook::new();
        // Initially, the book is empty, so spread is None
        assert_eq!(order_book.spread(), None);

        // Add a single side, spread should still be None
        order_book.add_limit_order(Side::Buy, Decimal::from(100), Decimal::from(10), 0);
        assert_eq!(order_book.spread(), None);

        // Add the other side, now spread should exist
        order_book.add_limit_order(Side::Sell, Decimal::from(105), Decimal::from(10), 1);
        assert_eq!(
            order_book.spread(),
            Some((Decimal::from(100), Decimal::from(105)))
        );

        // Narrow the spread
        order_book.add_limit_order(Side::Buy, Decimal::from(102), Decimal::from(10), 2);
        order_book.add_limit_order(Side::Sell, Decimal::from(104), Decimal::from(10), 3);
        assert_eq!(
            order_book.spread(),
            Some((Decimal::from(102), Decimal::from(104)))
        );

        // Remove the inner orders, spread should widen again
        let id1 = order_book.add_limit_order(Side::Buy, Decimal::from(103), Decimal::from(10), 4);
        assert_eq!(
            order_book.spread(),
            Some((Decimal::from(103), Decimal::from(104)))
        );
        order_book.cancel_order(id1).unwrap();
        assert_eq!(
            order_book.spread(),
            Some((Decimal::from(102), Decimal::from(104)))
        );
    }
}
