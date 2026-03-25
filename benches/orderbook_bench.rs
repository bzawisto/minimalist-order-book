use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use minimalist_order_book::{fixed_orderbook::FixedOrderBook, orderbook::OrderBook, types::Side};
use rand::prelude::*;
use rust_decimal::Decimal;

const ORDER_COUNT: usize = 10_000;
const SEED: u64 = 42;

fn generate_orders(levels: usize, count: usize, seed: u64) -> Vec<(Side, Decimal, Decimal)> {
    let mut rng = StdRng::seed_from_u64(seed);
    let base = Decimal::new(1000, 2); // 10.00
    (0..count)
        .map(|_| {
            let side = if rng.gen_bool(0.5) {
                Side::Buy
            } else {
                Side::Sell
            };
            let level = rng.gen_range(0..levels);
            let price = base + Decimal::new(level as i64, 2);
            let qty = Decimal::from(rng.gen_range(1u32..=100));
            (side, price, qty)
        })
        .collect()
}

/// Generate orders with prices normally distributed around the mid-level.
/// Simulates a realistic book where liquidity clusters near the spread.
fn generate_clustered_orders(
    levels: usize,
    count: usize,
    seed: u64,
) -> Vec<(Side, Decimal, Decimal)> {
    let mut rng = StdRng::seed_from_u64(seed);
    let base = Decimal::new(1000, 2); // 10.00
    let mid = levels as f64 / 2.0;
    let stddev = levels as f64 / 6.0; // ~99.7% within range
    (0..count)
        .map(|_| {
            let side = if rng.gen_bool(0.5) {
                Side::Buy
            } else {
                Side::Sell
            };
            // Box-Muller transform for normal distribution
            let u1: f64 = rng.gen_range(0.0001f64..1.0);
            let u2: f64 = rng.gen_range(0.0f64..1.0);
            let z = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos();
            let level = (mid + z * stddev).round().clamp(0.0, (levels - 1) as f64) as usize;
            let price = base + Decimal::new(level as i64, 2);
            let qty = Decimal::from(rng.gen_range(1u32..=100));
            (side, price, qty)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Insertion throughput (uniform)
// ---------------------------------------------------------------------------

fn bench_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("insert");

    for &levels in &[10usize, 100, 1000] {
        group.bench_function(BenchmarkId::new("dynamic", levels), |b| {
            b.iter_batched(
                || (generate_orders(levels, ORDER_COUNT, SEED), OrderBook::new()),
                |(orders, mut book)| {
                    for (i, (side, price, qty)) in orders.into_iter().enumerate() {
                        book.add_limit_order((i + 1) as u64, side, price, qty);
                    }
                    book // return so drop is excluded from timing
                },
                BatchSize::LargeInput,
            );
        });
    }

    // Fixed book requires const generic LEVELS — use a macro to instantiate each variant.
    // DEPTH=1024 (power of two, ~1000 orders per level capacity).
    macro_rules! bench_fixed_insert {
        ($levels:literal) => {
            group.bench_function(BenchmarkId::new("fixed", $levels), |b| {
                b.iter_batched(
                    || {
                        (
                            generate_orders($levels, ORDER_COUNT, SEED),
                            FixedOrderBook::<$levels, 1024>::new(
                                Decimal::new(1000, 2),
                                Decimal::new(1, 2),
                            ),
                        )
                    },
                    |(orders, mut book)| {
                        for (i, (side, price, qty)) in orders.into_iter().enumerate() {
                            let _ = book.add_limit_order((i + 1) as u64, side, price, qty);
                        }
                        book
                    },
                    BatchSize::LargeInput,
                );
            });
        };
    }

    bench_fixed_insert!(10);
    bench_fixed_insert!(100);
    bench_fixed_insert!(1000);

    group.finish();
}

// ---------------------------------------------------------------------------
// Cancellation throughput
// ---------------------------------------------------------------------------

fn bench_cancel(c: &mut Criterion) {
    let mut group = c.benchmark_group("cancel");

    for &levels in &[10usize, 100, 1000] {
        group.bench_function(BenchmarkId::new("dynamic", levels), |b| {
            b.iter_batched(
                || {
                    let orders = generate_orders(levels, ORDER_COUNT, SEED);
                    let mut book = OrderBook::new();
                    let ids: Vec<_> = orders
                        .iter()
                        .enumerate()
                        .map(|(i, &(side, price, qty))| {
                            book.add_limit_order((i + 1) as u64, side, price, qty)
                        })
                        .collect();
                    let mut shuffled = ids;
                    shuffled.shuffle(&mut StdRng::seed_from_u64(SEED + 1));
                    (book, shuffled)
                },
                |(mut book, ids)| {
                    for id in ids {
                        let _ = book.cancel_order(id);
                    }
                    book // return so drop is excluded from timing
                },
                BatchSize::LargeInput,
            );
        });
    }

    macro_rules! bench_fixed_cancel {
        ($levels:literal) => {
            group.bench_function(BenchmarkId::new("fixed", $levels), |b| {
                b.iter_batched(
                    || {
                        let orders = generate_orders($levels, ORDER_COUNT, SEED);
                        let mut book = FixedOrderBook::<$levels, 1024>::new(
                            Decimal::new(1000, 2),
                            Decimal::new(1, 2),
                        );
                        let ids: Vec<_> = orders
                            .iter()
                            .enumerate()
                            .filter_map(|(i, &(side, price, qty))| {
                                book.add_limit_order((i + 1) as u64, side, price, qty).ok()
                            })
                            .collect();
                        let mut shuffled = ids;
                        shuffled.shuffle(&mut StdRng::seed_from_u64(SEED + 1));
                        (book, shuffled)
                    },
                    |(mut book, ids)| {
                        for id in ids {
                            let _ = book.cancel_order(id);
                        }
                        book
                    },
                    BatchSize::LargeInput,
                );
            });
        };
    }

    bench_fixed_cancel!(10);
    bench_fixed_cancel!(100);
    bench_fixed_cancel!(1000);

    group.finish();
}

// ---------------------------------------------------------------------------
// Insertion throughput (clustered — normal distribution around mid-price)
// ---------------------------------------------------------------------------

fn bench_insert_clustered(c: &mut Criterion) {
    let mut group = c.benchmark_group("insert_clustered");

    for &levels in &[10usize, 100, 1000] {
        group.bench_function(BenchmarkId::new("dynamic", levels), |b| {
            b.iter_batched(
                || {
                    (
                        generate_clustered_orders(levels, ORDER_COUNT, SEED),
                        OrderBook::new(),
                    )
                },
                |(orders, mut book)| {
                    for (i, (side, price, qty)) in orders.into_iter().enumerate() {
                        book.add_limit_order((i + 1) as u64, side, price, qty);
                    }
                    book
                },
                BatchSize::LargeInput,
            );
        });
    }

    macro_rules! bench_fixed_insert_clustered {
        ($levels:literal) => {
            group.bench_function(BenchmarkId::new("fixed", $levels), |b| {
                b.iter_batched(
                    || {
                        (
                            generate_clustered_orders($levels, ORDER_COUNT, SEED),
                            FixedOrderBook::<$levels, 1024>::new(
                                Decimal::new(1000, 2),
                                Decimal::new(1, 2),
                            ),
                        )
                    },
                    |(orders, mut book)| {
                        for (i, (side, price, qty)) in orders.into_iter().enumerate() {
                            let _ = book.add_limit_order((i + 1) as u64, side, price, qty);
                        }
                        book
                    },
                    BatchSize::LargeInput,
                );
            });
        };
    }

    bench_fixed_insert_clustered!(10);
    bench_fixed_insert_clustered!(100);
    bench_fixed_insert_clustered!(1000);

    group.finish();
}

// ---------------------------------------------------------------------------
// Cancellation throughput (clustered)
// ---------------------------------------------------------------------------

fn bench_cancel_clustered(c: &mut Criterion) {
    let mut group = c.benchmark_group("cancel_clustered");

    for &levels in &[10usize, 100, 1000] {
        group.bench_function(BenchmarkId::new("dynamic", levels), |b| {
            b.iter_batched(
                || {
                    let orders = generate_clustered_orders(levels, ORDER_COUNT, SEED);
                    let mut book = OrderBook::new();
                    let ids: Vec<_> = orders
                        .iter()
                        .enumerate()
                        .map(|(i, &(side, price, qty))| {
                            book.add_limit_order((i + 1) as u64, side, price, qty)
                        })
                        .collect();
                    let mut shuffled = ids;
                    shuffled.shuffle(&mut StdRng::seed_from_u64(SEED + 1));
                    (book, shuffled)
                },
                |(mut book, ids)| {
                    for id in ids {
                        let _ = book.cancel_order(id);
                    }
                    book
                },
                BatchSize::LargeInput,
            );
        });
    }

    macro_rules! bench_fixed_cancel_clustered {
        ($levels:literal) => {
            group.bench_function(BenchmarkId::new("fixed", $levels), |b| {
                b.iter_batched(
                    || {
                        let orders = generate_clustered_orders($levels, ORDER_COUNT, SEED);
                        let mut book = FixedOrderBook::<$levels, 1024>::new(
                            Decimal::new(1000, 2),
                            Decimal::new(1, 2),
                        );
                        let ids: Vec<_> = orders
                            .iter()
                            .enumerate()
                            .filter_map(|(i, &(side, price, qty))| {
                                book.add_limit_order((i + 1) as u64, side, price, qty).ok()
                            })
                            .collect();
                        let mut shuffled = ids;
                        shuffled.shuffle(&mut StdRng::seed_from_u64(SEED + 1));
                        (book, shuffled)
                    },
                    |(mut book, ids)| {
                        for id in ids {
                            let _ = book.cancel_order(id);
                        }
                        book
                    },
                    BatchSize::LargeInput,
                );
            });
        };
    }

    bench_fixed_cancel_clustered!(10);
    bench_fixed_cancel_clustered!(100);
    bench_fixed_cancel_clustered!(1000);

    group.finish();
}

criterion_group!(
    benches,
    bench_insert,
    bench_cancel,
    bench_insert_clustered,
    bench_cancel_clustered
);
criterion_main!(benches);
