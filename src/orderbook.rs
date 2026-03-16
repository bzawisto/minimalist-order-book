use std::{
    cmp::Reverse,
    collections::{BTreeMap, HashMap},
    time::{SystemTime, UNIX_EPOCH},
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

    pub fn add_limit_order(&mut self, side: Side, price: Price, qty: Quantity) -> OrderId {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

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
    use super::*;

    #[test]
    fn test_add_and_cancel_order() {
        let mut ob = OrderBook::new();
        let id1 = ob.add_limit_order(Side::Buy, 100, 10);
        let id2 = ob.add_limit_order(Side::Sell, 105, 10);

        assert_eq!(ob.best_bid(), Some(100));
        assert_eq!(ob.best_ask(), Some(105));
        assert_eq!(ob.spread(), Some((100, 105)));

        assert!(ob.cancel_order(id1).is_ok());
        assert_eq!(ob.best_bid(), None);
        assert_eq!(ob.spread(), None);

        assert!(ob.cancel_order(id2).is_ok());
        assert_eq!(ob.best_ask(), None);
    }

    #[test]
    fn test_cancel_nonexistent_order() {
        let mut ob = OrderBook::new();
        assert!(ob.cancel_order(999).is_err());
    }

    #[test]
    fn test_best_bid_ask_ordering() {
        let mut ob = OrderBook::new();
        ob.add_limit_order(Side::Buy, 100, 10);
        ob.add_limit_order(Side::Buy, 110, 10);
        ob.add_limit_order(Side::Buy, 105, 10);
        assert_eq!(ob.best_bid(), Some(110));

        ob.add_limit_order(Side::Sell, 120, 10);
        ob.add_limit_order(Side::Sell, 115, 10);
        ob.add_limit_order(Side::Sell, 125, 10);
        assert_eq!(ob.best_ask(), Some(115));
    }

    #[test]
    fn test_spread() {
        let mut ob = OrderBook::new();
        // Initially, the book is empty, so spread is None
        assert_eq!(ob.spread(), None);

        // Add a single side, spread should still be None
        ob.add_limit_order(Side::Buy, 100, 10);
        assert_eq!(ob.spread(), None);

        // Add the other side, now spread should exist
        ob.add_limit_order(Side::Sell, 105, 10);
        assert_eq!(ob.spread(), Some((100, 105)));

        // Narrow the spread
        ob.add_limit_order(Side::Buy, 102, 10);
        ob.add_limit_order(Side::Sell, 104, 10);
        assert_eq!(ob.spread(), Some((102, 104)));

        // Remove the inner orders, spread should widen again
        let id1 = ob.add_limit_order(Side::Buy, 103, 10);
        assert_eq!(ob.spread(), Some((103, 104)));
        ob.cancel_order(id1).unwrap();
        assert_eq!(ob.spread(), Some((102, 104)));
    }
}
