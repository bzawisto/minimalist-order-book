use std::error::Error;

use minimalist_order_book::{orderbook::OrderBook, types::Side};

fn main() -> Result<(), Box<dyn Error>> {
    println!("--- Minimalist Order Book Demo ---");
    let mut ob = OrderBook::new();

    println!("Adding Buy order: 100 @ $5000");
    let _id1 = ob.add_limit_order(Side::Buy, 5000, 100);

    println!("Adding Sell order: 50 @ $5100");
    let _id2 = ob.add_limit_order(Side::Sell, 5100, 50);

    println!("Adding Sell order: 10 @ $5050");
    let id3 = ob.add_limit_order(Side::Sell, 5050, 10);

    println!("\nCurrent Book State:");
    println!("Best Bid: {:?}", ob.best_bid());
    println!("Best Ask: {:?}", ob.best_ask());
    println!("Spread: {:?}", ob.spread());

    println!("\nCanceling active Sell order $5050 (ID: {id3})");
    ob.cancel_order(id3)?;

    println!("\nUpdated Book State:");
    println!("Best Bid: {:?}", ob.best_bid());
    println!("Best Ask: {:?}", ob.best_ask());
    println!("Spread: {:?}", ob.spread());

    Ok(())
}
