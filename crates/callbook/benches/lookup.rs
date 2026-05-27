//! Modern 2025 `ham0` lookup benchmarks.
//!
//! These benches require a real licensed database and are skipped unless
//! `CALLBOOK_DB` points at the `ham0` directory.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use callbook::{CallBook, LookupStatus};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

const CURRENT_US: &str = "W1AW";
const INTERNATIONAL: &str = "S51DX";
const NOT_FOUND: &str = "NOPE9999";
const MIXED_CALLS: &[&str] = &["W1AW", "S51DX", "PA0RDT", "NOPE9999"];
const CURRENT_US_SAMPLE_SIZE: usize = 64;
const EXCLUDED_CURRENT_US_SAMPLE_CALLS: &[&str] = &["W1AW"];

fn db_path() -> Option<PathBuf> {
    std::env::var_os("CALLBOOK_DB").map(PathBuf::from)
}

fn open_db() -> Option<CallBook> {
    let Some(path) = db_path() else {
        eprintln!("skipping modern lookup benchmark: CALLBOOK_DB is not set");
        return None;
    };
    match CallBook::open(&path) {
        Ok(db) => Some(db),
        Err(err) => {
            eprintln!(
                "skipping modern lookup benchmark: failed to open {}: {err}",
                path.display()
            );
            None
        }
    }
}

fn bench_open(c: &mut Criterion) {
    let Some(path) = db_path() else {
        eprintln!("skipping CallBook::open benchmark: CALLBOOK_DB is not set");
        return;
    };

    c.bench_function("modern_open_build_indexes", |b| {
        b.iter(|| {
            let db = CallBook::open(black_box(&path)).expect("open CALLBOOK_DB");
            black_box(db);
        });
    });
}

fn bench_warm_lookup(c: &mut Criterion) {
    let Some(db) = open_db() else {
        return;
    };

    let mut group = c.benchmark_group("modern_lookup_warm");
    for callsign in [CURRENT_US, INTERNATIONAL, NOT_FOUND] {
        group.throughput(Throughput::Elements(1));
        group.bench_with_input(
            BenchmarkId::from_parameter(callsign),
            callsign,
            |b, call| {
                b.iter(|| {
                    let result = db.lookup(black_box(call)).expect("lookup");
                    black_box(result);
                });
            },
        );
    }
    group.finish();
}

fn bench_mixed_batch(c: &mut Criterion) {
    let Some(db) = open_db() else {
        return;
    };

    let mut group = c.benchmark_group("modern_lookup_mixed_batch");
    group.throughput(Throughput::Elements(MIXED_CALLS.len() as u64));
    group.bench_function("mixed_calls", |b| {
        let mut batch = db.batch_lookup();
        b.iter(|| {
            let mut found = 0usize;
            for call in black_box(MIXED_CALLS) {
                let result = batch.lookup(call).expect("lookup");
                if !matches!(result.status(), LookupStatus::NotFound) {
                    found += 1;
                }
            }
            black_box(found);
        });
    });
    group.finish();
}

fn bench_current_us_catalog_lookup(c: &mut Criterion) {
    let Some(db) = open_db() else {
        return;
    };
    let Some(catalog) = db.current_us_catalog() else {
        eprintln!("skipping current-US catalog benchmark: usa.csv.zip is not loaded");
        return;
    };
    let hits = sample_current_us_calls(catalog, CURRENT_US_SAMPLE_SIZE);
    if hits.is_empty() {
        eprintln!("skipping current-US catalog benchmark: no sampleable callsigns");
        return;
    }
    let mixed = mixed_current_us_calls(&hits);

    let mut group = c.benchmark_group("current_us_catalog_lookup");
    group.throughput(Throughput::Elements(hits.len() as u64));
    group.bench_function("get_random_hits", |b| {
        b.iter(|| {
            let mut found = 0usize;
            for call in black_box(&hits) {
                if catalog.get(call).is_some() {
                    found += 1;
                }
            }
            black_box(found);
        });
    });
    group.bench_function("contains_random_hits", |b| {
        b.iter(|| {
            let mut found = 0usize;
            for call in black_box(&hits) {
                if catalog.contains_callsign(call) {
                    found += 1;
                }
            }
            black_box(found);
        });
    });
    group.bench_function("lookup_owned_random_hits", |b| {
        b.iter(|| {
            let mut found = 0usize;
            for call in black_box(&hits) {
                if catalog.lookup(call).expect("current-US lookup").is_some() {
                    found += 1;
                }
            }
            black_box(found);
        });
    });
    group.throughput(Throughput::Elements(mixed.len() as u64));
    group.bench_function("contains_random_mixed", |b| {
        b.iter(|| {
            let mut found = 0usize;
            for call in black_box(&mixed) {
                if catalog.contains_callsign(call) {
                    found += 1;
                }
            }
            black_box(found);
        });
    });
    group.finish();
}

fn bench_parallel_batch(c: &mut Criterion) {
    let Some(db) = open_db() else {
        return;
    };
    let db = Arc::new(db);

    let mut group = c.benchmark_group("modern_lookup_parallel_batch");
    group.throughput(Throughput::Elements((MIXED_CALLS.len() * 4) as u64));
    group.bench_function("four_threads_x_mixed_calls", |b| {
        b.iter(|| {
            std::thread::scope(|scope| {
                let handles: Vec<_> = (0..4)
                    .map(|_| {
                        let db = Arc::clone(&db);
                        scope.spawn(move || {
                            let mut found = 0usize;
                            for call in black_box(MIXED_CALLS) {
                                let result = db.lookup(call).expect("lookup");
                                if !matches!(result.status(), LookupStatus::NotFound) {
                                    found += 1;
                                }
                            }
                            found
                        })
                    })
                    .collect();
                let found: usize = handles
                    .into_iter()
                    .map(|handle| handle.join().expect("lookup thread"))
                    .sum();
                black_box(found);
            });
        });
    });
    group.finish();
}

fn sample_current_us_calls(catalog: &callbook::UsCsvFile, target_len: usize) -> Vec<String> {
    let candidates = catalog
        .callsigns()
        .filter(|callsign| {
            !EXCLUDED_CURRENT_US_SAMPLE_CALLS
                .iter()
                .any(|excluded| callsign.eq_ignore_ascii_case(excluded))
        })
        .collect::<Vec<_>>();
    let target_len = target_len.min(candidates.len());
    let mut seed = runtime_seed();
    let mut selected = Vec::with_capacity(target_len);
    let mut selected_indices = Vec::with_capacity(target_len);
    let mut attempts = 0usize;
    while selected.len() < target_len && attempts < target_len.saturating_mul(32) {
        attempts += 1;
        seed = xorshift64(seed);
        let index = (seed as usize) % candidates.len();
        if selected_indices.contains(&index) {
            continue;
        }
        selected_indices.push(index);
        selected.push(candidates[index].to_owned());
    }
    selected
}

fn mixed_current_us_calls(hits: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(hits.len() * 2);
    for (index, call) in hits.iter().enumerate() {
        out.push(call.clone());
        out.push(format!("ZZZ{index:08}{len}", len = call.len()));
    }
    out
}

fn runtime_seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0x9e3779b97f4a7c15, |duration| {
            duration.as_nanos() as u64 ^ 0x9e3779b97f4a7c15
        })
}

fn xorshift64(mut value: u64) -> u64 {
    if value == 0 {
        value = 0x9e3779b97f4a7c15;
    }
    value ^= value << 13;
    value ^= value >> 7;
    value ^ (value << 17)
}

criterion_group!(
    benches,
    bench_open,
    bench_warm_lookup,
    bench_mixed_batch,
    bench_current_us_catalog_lookup,
    bench_parallel_batch
);
criterion_main!(benches);
