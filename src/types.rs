use rust_decimal::Decimal;

pub type Price = Decimal;
pub type Quantity = Decimal;
pub type OrderId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}
