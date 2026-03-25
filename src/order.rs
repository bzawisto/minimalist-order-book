use crate::types::{OrderId, Price, Quantity};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Order {
    pub id: OrderId,
    pub price: Price, // 0 for market orders
    pub quantity: Quantity,
}
