use std::error::Error;

use minimalist_order_book::{orderbook::OrderBook, types::Side};
use rust_decimal::Decimal;

fn main() -> Result<(), Box<dyn Error>> {
    println!("--- Minimalist Order Book Demo ---");
    let mut order_book = OrderBook::new();

    println!("Adding Buy order: 100 @ $5000");
    let _id1 = order_book.add_limit_order(1, Side::Buy, Decimal::from(5000), Decimal::from(100));

    println!("Adding Sell order: 50 @ $5100");
    let _id2 = order_book.add_limit_order(2, Side::Sell, Decimal::from(5100), Decimal::from(50));

    println!("Adding Sell order: 10 @ $5050");
    let id3 = order_book.add_limit_order(3, Side::Sell, Decimal::from(5050), Decimal::from(10));

    println!("\nCurrent Book State:");
    println!("Best Bid: {:?}", order_book.best_bid());
    println!("Best Ask: {:?}", order_book.best_ask());
    println!("Spread: {:?}", order_book.spread());

    println!("\nCanceling active Sell order $5050 (ID: {id3})");
    order_book.cancel_order(id3)?;

    println!("\nUpdated Book State:");
    println!("Best Bid: {:?}", order_book.best_bid());
    println!("Best Ask: {:?}", order_book.best_ask());
    println!("Spread: {:?}", order_book.spread());

    Ok(())
}
