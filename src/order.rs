use crate::types::{OrderId, Price, Quantity, Side};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Order {
    pub id: OrderId,
    pub side: Side,
    pub price: Price, // 0 for market orders
    pub quantity: Quantity,
    pub remaining: Quantity,
    pub timestamp: u64,
}
