use std::{
    cmp::Reverse,
    collections::{BTreeMap, HashMap},
};

use rust_decimal::Decimal;
use thiserror::Error;

use crate::{
    order::Order,
    types::{Fill, OrderId, Price, Quantity, Side},
};

#[derive(Error, Debug)]
pub enum OrderError {
    #[error("Order ID {0} not found")]
    NotFound(OrderId),
}

pub struct OrderBook {
    // Highest price first for bids
    pub bids: BTreeMap<Reverse<Price>, Vec<Order>>,
    // Lowest price first for asks
    pub asks: BTreeMap<Price, Vec<Order>>,
    // Index to quickly find an order by its ID
    pub order_index: HashMap<OrderId, Price>,
}

impl OrderBook {
    pub fn new() -> Self {
        Self {
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            order_index: HashMap::new(),
        }
    }

    pub fn add_limit_order(
        &mut self,
        id: OrderId,
        side: Side,
        price: Price,
        qty: Quantity,
    ) -> Vec<Fill> {
        let (remaining, fills) = self.match_incoming(id, side, price, qty);

        if remaining > Decimal::ZERO {
            let order = Order {
                id,
                price,
                quantity: remaining,
            };
            self.order_index.insert(id, price);
            match side {
                Side::Buy => self.bids.entry(Reverse(price)).or_default().push(order),
                Side::Sell => self.asks.entry(price).or_default().push(order),
            }
        }

        fills
    }

    pub fn add_market_order(&mut self, id: OrderId, side: Side, qty: Quantity) -> Vec<Fill> {
        let price = match side {
            Side::Buy => Decimal::MAX,
            Side::Sell => Decimal::ZERO,
        };
        self.match_incoming(id, side, price, qty).1
    }

    fn match_incoming(
        &mut self,
        id: OrderId,
        side: Side,
        price: Price,
        qty: Quantity,
    ) -> (Quantity, Vec<Fill>) {
        let mut remaining = qty;
        let mut fills = Vec::new();
        let Self {
            bids,
            asks,
            order_index,
        } = self;

        match side {
            Side::Buy => {
                while remaining > Decimal::ZERO {
                    let ask_price = match asks.first_key_value() {
                        Some((&p, _)) => p,
                        None => break,
                    };
                    if ask_price > price {
                        break;
                    }

                    let level = asks.get_mut(&ask_price).unwrap();
                    while remaining > Decimal::ZERO && !level.is_empty() {
                        let fill_qty = remaining.min(level[0].quantity);
                        fills.push(Fill {
                            maker_order_id: level[0].id,
                            taker_order_id: id,
                            price: ask_price,
                            quantity: fill_qty,
                        });
                        level[0].quantity -= fill_qty;
                        remaining -= fill_qty;
                        if level[0].quantity.is_zero() {
                            order_index.remove(&level[0].id);
                            level.remove(0);
                        }
                    }
                    let level_empty = level.is_empty();
                    if level_empty {
                        asks.remove(&ask_price);
                    }
                }
            }
            Side::Sell => {
                while remaining > Decimal::ZERO {
                    let bid_price = match bids.first_key_value() {
                        Some((rev, _)) => rev.0,
                        None => break,
                    };
                    if bid_price < price {
                        break;
                    }

                    let level = bids.get_mut(&Reverse(bid_price)).unwrap();
                    while remaining > Decimal::ZERO && !level.is_empty() {
                        let fill_qty = remaining.min(level[0].quantity);
                        fills.push(Fill {
                            maker_order_id: level[0].id,
                            taker_order_id: id,
                            price: bid_price,
                            quantity: fill_qty,
                        });
                        level[0].quantity -= fill_qty;
                        remaining -= fill_qty;
                        if level[0].quantity.is_zero() {
                            order_index.remove(&level[0].id);
                            level.remove(0);
                        }
                    }
                    let level_empty = level.is_empty();
                    if level_empty {
                        bids.remove(&Reverse(bid_price));
                    }
                }
            }
        }

        (remaining, fills)
    }

    pub fn cancel_order(&mut self, id: OrderId) -> Result<(), OrderError> {
        let price = self
            .order_index
            .remove(&id)
            .ok_or(OrderError::NotFound(id))?;

        if let Some(list) = self.bids.get_mut(&Reverse(price))
            && let Some(pos) = list.iter().position(|o| o.id == id)
        {
            list.copy_within((pos + 1).., pos);
            list.pop();

            if list.is_empty() {
                self.bids.remove(&Reverse(price));
            }
            return Ok(());
        }

        if let Some(list) = self.asks.get_mut(&price)
            && let Some(pos) = list.iter().position(|o| o.id == id)
        {
            list.copy_within((pos + 1).., pos);
            list.pop();

            if list.is_empty() {
                self.asks.remove(&price);
            }
            return Ok(());
        }

        Err(OrderError::NotFound(id))
    }

    pub fn best_bid(&self) -> Option<Price> {
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
        let mut book = OrderBook::new();
        book.add_limit_order(1, Side::Buy, Decimal::from(100), Decimal::from(10));
        book.add_limit_order(2, Side::Sell, Decimal::from(105), Decimal::from(10));

        assert_eq!(book.best_bid(), Some(Decimal::from(100)));
        assert_eq!(book.best_ask(), Some(Decimal::from(105)));
        assert_eq!(
            book.spread(),
            Some((Decimal::from(100), Decimal::from(105)))
        );

        assert!(book.cancel_order(1).is_ok());
        assert_eq!(book.best_bid(), None);
        assert_eq!(book.spread(), None);

        assert!(book.cancel_order(2).is_ok());
        assert_eq!(book.best_ask(), None);
    }

    #[test]
    fn test_cancel_nonexistent_order() {
        let mut book = OrderBook::new();
        assert!(book.cancel_order(999).is_err());
    }

    #[test]
    fn test_best_bid_ask_ordering() {
        let mut book = OrderBook::new();
        book.add_limit_order(1, Side::Buy, Decimal::from(100), Decimal::from(10));
        book.add_limit_order(2, Side::Buy, Decimal::from(110), Decimal::from(10));
        book.add_limit_order(3, Side::Buy, Decimal::from(105), Decimal::from(10));
        assert_eq!(book.best_bid(), Some(Decimal::from(110)));

        book.add_limit_order(4, Side::Sell, Decimal::from(120), Decimal::from(10));
        book.add_limit_order(5, Side::Sell, Decimal::from(115), Decimal::from(10));
        book.add_limit_order(6, Side::Sell, Decimal::from(125), Decimal::from(10));
        assert_eq!(book.best_ask(), Some(Decimal::from(115)));
    }

    #[test]
    fn test_spread() {
        let mut book = OrderBook::new();
        assert_eq!(book.spread(), None);

        book.add_limit_order(1, Side::Buy, Decimal::from(100), Decimal::from(10));
        assert_eq!(book.spread(), None);

        book.add_limit_order(2, Side::Sell, Decimal::from(105), Decimal::from(10));
        assert_eq!(
            book.spread(),
            Some((Decimal::from(100), Decimal::from(105)))
        );

        book.add_limit_order(3, Side::Buy, Decimal::from(102), Decimal::from(10));
        book.add_limit_order(4, Side::Sell, Decimal::from(104), Decimal::from(10));
        assert_eq!(
            book.spread(),
            Some((Decimal::from(102), Decimal::from(104)))
        );

        book.add_limit_order(5, Side::Buy, Decimal::from(103), Decimal::from(10));
        assert_eq!(
            book.spread(),
            Some((Decimal::from(103), Decimal::from(104)))
        );
        book.cancel_order(5).unwrap();
        assert_eq!(
            book.spread(),
            Some((Decimal::from(102), Decimal::from(104)))
        );
    }

    #[test]
    fn test_exact_fill() {
        let mut book = OrderBook::new();
        book.add_limit_order(1, Side::Sell, Decimal::from(100), Decimal::from(10));
        let fills = book.add_limit_order(2, Side::Buy, Decimal::from(100), Decimal::from(10));
        assert_eq!(fills.len(), 1);
        assert_eq!(
            fills[0],
            Fill {
                maker_order_id: 1,
                taker_order_id: 2,
                price: Decimal::from(100),
                quantity: Decimal::from(10),
            }
        );
        assert_eq!(book.best_bid(), None);
        assert_eq!(book.best_ask(), None);
    }

    #[test]
    fn test_partial_fill_taker_remains() {
        let mut book = OrderBook::new();
        book.add_limit_order(1, Side::Sell, Decimal::from(100), Decimal::from(5));
        let fills = book.add_limit_order(2, Side::Buy, Decimal::from(100), Decimal::from(10));
        assert_eq!(fills.len(), 1);
        assert_eq!(fills[0].quantity, Decimal::from(5));
        assert_eq!(book.best_ask(), None);
        // 5 remaining rests as a bid
        assert_eq!(book.best_bid(), Some(Decimal::from(100)));
    }

    #[test]
    fn test_partial_fill_maker_remains() {
        let mut book = OrderBook::new();
        book.add_limit_order(1, Side::Sell, Decimal::from(100), Decimal::from(10));
        let fills = book.add_limit_order(2, Side::Buy, Decimal::from(100), Decimal::from(5));
        assert_eq!(fills.len(), 1);
        assert_eq!(fills[0].quantity, Decimal::from(5));
        // 5 remaining in the ask
        assert_eq!(book.best_ask(), Some(Decimal::from(100)));
        assert_eq!(book.best_bid(), None);
    }

    #[test]
    fn test_multi_level_sweep() {
        let mut book = OrderBook::new();
        book.add_limit_order(1, Side::Sell, Decimal::from(100), Decimal::from(5));
        book.add_limit_order(2, Side::Sell, Decimal::from(101), Decimal::from(5));
        book.add_limit_order(3, Side::Sell, Decimal::from(102), Decimal::from(5));

        let fills = book.add_limit_order(4, Side::Buy, Decimal::from(102), Decimal::from(12));
        assert_eq!(fills.len(), 3);
        assert_eq!(fills[0].maker_order_id, 1);
        assert_eq!(fills[0].price, Decimal::from(100));
        assert_eq!(fills[0].quantity, Decimal::from(5));
        assert_eq!(fills[1].maker_order_id, 2);
        assert_eq!(fills[1].price, Decimal::from(101));
        assert_eq!(fills[1].quantity, Decimal::from(5));
        assert_eq!(fills[2].maker_order_id, 3);
        assert_eq!(fills[2].price, Decimal::from(102));
        assert_eq!(fills[2].quantity, Decimal::from(2));
        // 3 remaining in ask at 102
        assert_eq!(book.best_ask(), Some(Decimal::from(102)));
        assert_eq!(book.best_bid(), None);
    }

    #[test]
    fn test_price_time_priority() {
        let mut book = OrderBook::new();
        book.add_limit_order(1, Side::Sell, Decimal::from(100), Decimal::from(5));
        book.add_limit_order(2, Side::Sell, Decimal::from(100), Decimal::from(5));

        let fills = book.add_limit_order(3, Side::Buy, Decimal::from(100), Decimal::from(7));
        assert_eq!(fills.len(), 2);
        // Order 1 was placed first, fills first (price-time priority)
        assert_eq!(fills[0].maker_order_id, 1);
        assert_eq!(fills[0].quantity, Decimal::from(5));
        assert_eq!(fills[1].maker_order_id, 2);
        assert_eq!(fills[1].quantity, Decimal::from(2));
    }

    #[test]
    fn test_no_match_buy_below_asks() {
        let mut book = OrderBook::new();
        book.add_limit_order(1, Side::Sell, Decimal::from(105), Decimal::from(10));
        let fills = book.add_limit_order(2, Side::Buy, Decimal::from(100), Decimal::from(10));
        assert!(fills.is_empty());
        assert_eq!(book.best_bid(), Some(Decimal::from(100)));
        assert_eq!(book.best_ask(), Some(Decimal::from(105)));
    }

    #[test]
    fn test_sell_matches_bids() {
        let mut book = OrderBook::new();
        book.add_limit_order(1, Side::Buy, Decimal::from(100), Decimal::from(10));
        let fills = book.add_limit_order(2, Side::Sell, Decimal::from(100), Decimal::from(10));
        assert_eq!(fills.len(), 1);
        assert_eq!(fills[0].maker_order_id, 1);
        assert_eq!(fills[0].taker_order_id, 2);
        assert_eq!(book.best_bid(), None);
        assert_eq!(book.best_ask(), None);
    }

    #[test]
    fn test_sell_sweeps_multiple_bid_levels() {
        let mut book = OrderBook::new();
        book.add_limit_order(1, Side::Buy, Decimal::from(102), Decimal::from(5));
        book.add_limit_order(2, Side::Buy, Decimal::from(101), Decimal::from(5));
        book.add_limit_order(3, Side::Buy, Decimal::from(100), Decimal::from(5));

        let fills = book.add_limit_order(4, Side::Sell, Decimal::from(100), Decimal::from(12));
        assert_eq!(fills.len(), 3);
        // Fills at highest bid first
        assert_eq!(fills[0].maker_order_id, 1);
        assert_eq!(fills[0].price, Decimal::from(102));
        assert_eq!(fills[1].maker_order_id, 2);
        assert_eq!(fills[1].price, Decimal::from(101));
        assert_eq!(fills[2].maker_order_id, 3);
        assert_eq!(fills[2].price, Decimal::from(100));
        assert_eq!(fills[2].quantity, Decimal::from(2));
    }

    #[test]
    fn test_market_buy() {
        let mut book = OrderBook::new();
        book.add_limit_order(1, Side::Sell, Decimal::from(100), Decimal::from(10));
        book.add_limit_order(2, Side::Sell, Decimal::from(200), Decimal::from(10));

        let fills = book.add_market_order(3, Side::Buy, Decimal::from(15));
        assert_eq!(fills.len(), 2);
        assert_eq!(fills[0].price, Decimal::from(100));
        assert_eq!(fills[0].quantity, Decimal::from(10));
        assert_eq!(fills[1].price, Decimal::from(200));
        assert_eq!(fills[1].quantity, Decimal::from(5));
        // Market order never rests
        assert_eq!(book.best_bid(), None);
    }

    #[test]
    fn test_market_sell() {
        let mut book = OrderBook::new();
        book.add_limit_order(1, Side::Buy, Decimal::from(100), Decimal::from(10));

        let fills = book.add_market_order(2, Side::Sell, Decimal::from(10));
        assert_eq!(fills.len(), 1);
        assert_eq!(fills[0].price, Decimal::from(100));
        assert_eq!(book.best_bid(), None);
    }

    #[test]
    fn test_market_order_insufficient_liquidity() {
        let mut book = OrderBook::new();
        book.add_limit_order(1, Side::Sell, Decimal::from(100), Decimal::from(5));

        let fills = book.add_market_order(2, Side::Buy, Decimal::from(10));
        assert_eq!(fills.len(), 1);
        assert_eq!(fills[0].quantity, Decimal::from(5));
        // Unfilled portion is discarded, not rested
        assert_eq!(book.best_bid(), None);
        assert_eq!(book.best_ask(), None);
    }
}
