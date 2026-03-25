use crate::types::{OrderId, Price, Quantity};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Order {
    pub id: OrderId,
    pub price: Price,
    pub quantity: Quantity,
}
