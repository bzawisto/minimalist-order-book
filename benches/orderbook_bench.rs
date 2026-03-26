use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use minimalist_order_book::{fixed_orderbook::FixedOrderBook, orderbook::OrderBook, types::Side};
use rand::prelude::*;
use rust_decimal::Decimal;

const ORDER_COUNT: usize = 10_000;
const SEED: u64 = 42;

/// Generate non-crossing orders: buys in the lower half, sells in the upper half.
/// This ensures no matching occurs so benchmarks measure pure insert/cancel throughput.
fn generate_orders(levels: usize, count: usize, seed: u64) -> Vec<(Side, Decimal, Decimal)> {
    let mut rng = StdRng::seed_from_u64(seed);
    let base = Decimal::new(1000, 2); // 10.00
    let mid = levels / 2;
    (0..count)
        .map(|_| {
            let side = if rng.gen_bool(0.5) {
                Side::Buy
            } else {
                Side::Sell
            };
            let level = match side {
                Side::Buy => rng.gen_range(0..mid.max(1)),
                Side::Sell => rng.gen_range(mid..levels),
            };
            let price = base + Decimal::new(level as i64, 2);
            let qty = Decimal::from(rng.gen_range(1u32..=100));
            (side, price, qty)
        })
        .collect()
}

/// Generate non-crossing orders with prices normally distributed around each side's midpoint.
/// Buys cluster in the lower half, sells in the upper half — no matching occurs.
fn generate_clustered_orders(
    levels: usize,
    count: usize,
    seed: u64,
) -> Vec<(Side, Decimal, Decimal)> {
    let mut rng = StdRng::seed_from_u64(seed);
    let base = Decimal::new(1000, 2); // 10.00
    let half = levels / 2;
    (0..count)
        .map(|_| {
            let side = if rng.gen_bool(0.5) {
                Side::Buy
            } else {
                Side::Sell
            };
            // Each side clusters around its own midpoint
            let (lo, hi) = match side {
                Side::Buy => (0usize, half.max(1)),
                Side::Sell => (half, levels),
            };
            let range = hi - lo;
            let mid = lo as f64 + range as f64 / 2.0;
            let stddev = range as f64 / 6.0;
            // Box-Muller transform for normal distribution
            let u1: f64 = rng.gen_range(0.0001f64..1.0);
            let u2: f64 = rng.gen_range(0.0f64..1.0);
            let z = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos();
            let level = (mid + z * stddev).round().clamp(lo as f64, (hi - 1) as f64) as usize;
            let price = base + Decimal::new(level as i64, 2);
            let qty = Decimal::from(rng.gen_range(1u32..=100));
            (side, price, qty)
        })
        .collect()
}

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

fn bench_cancel(c: &mut Criterion) {
    let mut group = c.benchmark_group("cancel");

    for &levels in &[10usize, 100, 1000] {
        group.bench_function(BenchmarkId::new("dynamic", levels), |b| {
            b.iter_batched(
                || {
                    let orders = generate_orders(levels, ORDER_COUNT, SEED);
                    let mut book = OrderBook::new();
                    let mut ids = Vec::with_capacity(ORDER_COUNT);
                    for (i, &(side, price, qty)) in orders.iter().enumerate() {
                        let id = (i + 1) as u64;
                        book.add_limit_order(id, side, price, qty);
                        ids.push(id);
                    }
                    ids.shuffle(&mut StdRng::seed_from_u64(SEED + 1));
                    (book, ids)
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
                        let mut ids = Vec::with_capacity(ORDER_COUNT);
                        for (i, &(side, price, qty)) in orders.iter().enumerate() {
                            let id = (i + 1) as u64;
                            if book.add_limit_order(id, side, price, qty).is_ok() {
                                ids.push(id);
                            }
                        }
                        ids.shuffle(&mut StdRng::seed_from_u64(SEED + 1));
                        (book, ids)
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

fn bench_cancel_clustered(c: &mut Criterion) {
    let mut group = c.benchmark_group("cancel_clustered");

    for &levels in &[10usize, 100, 1000] {
        group.bench_function(BenchmarkId::new("dynamic", levels), |b| {
            b.iter_batched(
                || {
                    let orders = generate_clustered_orders(levels, ORDER_COUNT, SEED);
                    let mut book = OrderBook::new();
                    let mut ids = Vec::with_capacity(ORDER_COUNT);
                    for (i, &(side, price, qty)) in orders.iter().enumerate() {
                        let id = (i + 1) as u64;
                        book.add_limit_order(id, side, price, qty);
                        ids.push(id);
                    }
                    ids.shuffle(&mut StdRng::seed_from_u64(SEED + 1));
                    (book, ids)
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
                        let mut ids = Vec::with_capacity(ORDER_COUNT);
                        for (i, &(side, price, qty)) in orders.iter().enumerate() {
                            let id = (i + 1) as u64;
                            if book.add_limit_order(id, side, price, qty).is_ok() {
                                ids.push(id);
                            }
                        }
                        ids.shuffle(&mut StdRng::seed_from_u64(SEED + 1));
                        (book, ids)
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
