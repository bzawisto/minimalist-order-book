use std::error::Error;

use minimalist_order_book::{orderbook::OrderBook, types::Side};
use rust_decimal::Decimal;

fn main() -> Result<(), Box<dyn Error>> {
    println!("--- Minimalist Order Book Demo ---");
    let mut book = OrderBook::new();

    println!("Adding Buy order: 100 @ $5000");
    book.add_limit_order(1, Side::Buy, Decimal::from(5000), Decimal::from(100));

    println!("Adding Sell order: 50 @ $5100");
    book.add_limit_order(2, Side::Sell, Decimal::from(5100), Decimal::from(50));

    println!("Adding Sell order: 10 @ $5050");
    book.add_limit_order(3, Side::Sell, Decimal::from(5050), Decimal::from(10));

    println!("\nCurrent Book State:");
    println!("Best Bid: {:?}", book.best_bid());
    println!("Best Ask: {:?}", book.best_ask());
    println!("Spread: {:?}", book.spread());

    println!("\nSending aggressive Buy 200 @ $5100 (crosses both asks)");
    let fills = book.add_limit_order(4, Side::Buy, Decimal::from(5100), Decimal::from(200));
    for fill in &fills {
        println!(
            "  Fill: {} @ {} (maker={}, taker={})",
            fill.quantity, fill.price, fill.maker_order_id, fill.taker_order_id
        );
    }

    println!("\nUpdated Book State:");
    println!("Best Bid: {:?}", book.best_bid());
    println!("Best Ask: {:?}", book.best_ask());
    println!("Spread: {:?}", book.spread());

    println!("\nCanceling resting bid (ID: 4)");
    book.cancel_order(4)?;
    println!("Best Bid: {:?}", book.best_bid());

    Ok(())
}
