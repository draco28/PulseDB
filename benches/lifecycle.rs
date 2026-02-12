//! Benchmarks for PulseDB database lifecycle operations.
//!
//! Run with: `cargo bench`
//!
//! Performance targets:
//! - `open()` < 100ms for new database
//! - `open()` < 100ms for existing database with 100K experiences
//! - `close()` < 50ms

use criterion::{criterion_group, criterion_main, Criterion};
use pulsedb::{Config, PulseDB};
use tempfile::tempdir;

/// Benchmark opening a new database.
fn bench_open_new(c: &mut Criterion) {
    c.bench_function("open_new_database", |b| {
        b.iter_custom(|iters| {
            let mut total = std::time::Duration::ZERO;

            for _ in 0..iters {
                let dir = tempdir().unwrap();
                let path = dir.path().join("test.db");

                let start = std::time::Instant::now();
                let db = PulseDB::open(&path, Config::default()).unwrap();
                total += start.elapsed();

                db.close().unwrap();
            }

            total
        });
    });
}

/// Benchmark opening an existing database.
fn bench_open_existing(c: &mut Criterion) {
    let dir = tempdir().unwrap();
    let path = dir.path().join("test.db");

    // Create database first
    let db = PulseDB::open(&path, Config::default()).unwrap();
    db.close().unwrap();

    c.bench_function("open_existing_database", |b| {
        b.iter(|| {
            let db = PulseDB::open(&path, Config::default()).unwrap();
            db.close().unwrap();
        });
    });
}

/// Benchmark closing a database.
fn bench_close(c: &mut Criterion) {
    c.bench_function("close_database", |b| {
        b.iter_custom(|iters| {
            let mut total = std::time::Duration::ZERO;

            for _ in 0..iters {
                let dir = tempdir().unwrap();
                let path = dir.path().join("test.db");

                let db = PulseDB::open(&path, Config::default()).unwrap();

                let start = std::time::Instant::now();
                db.close().unwrap();
                total += start.elapsed();
            }

            total
        });
    });
}

criterion_group!(benches, bench_open_new, bench_open_existing, bench_close);
criterion_main!(benches);
