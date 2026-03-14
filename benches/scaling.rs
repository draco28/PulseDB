//! Scaling benchmarks for PulseDB operations at increasing data sizes.
//!
//! Run with: `cargo bench -- scaling`
//!
//! Tests performance at 1K, 10K, and 100K experiences.
//! The 100K benchmark takes several minutes to set up — run separately
//! from quick benchmarks during development.
//!
//! Quick validation (1K only):
//!   `cargo bench -- scaling/1000`

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use pulsedb::{CollectiveId, Config, NewExperience, PulseDB};
use std::time::Duration;
use tempfile::tempdir;

/// Default embedding dimension (D384).
const DIM: usize = 384;

/// Generates a deterministic embedding from a seed.
fn make_embedding(seed: u64) -> Vec<f32> {
    (0..DIM)
        .map(|i| {
            let h = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(i as u64)
                .wrapping_mul(1442695040888963407);
            (h >> 33) as f32 / (u32::MAX as f32) - 0.5
        })
        .collect()
}

/// Sets up a database pre-populated with `n` experiences.
fn setup_db_with_n(n: usize) -> (PulseDB, CollectiveId, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let db = PulseDB::open(dir.path().join("bench.db"), Config::default()).unwrap();
    let cid = db.create_collective("bench").unwrap();

    for i in 0..n as u64 {
        db.record_experience(NewExperience {
            collective_id: cid,
            content: format!("Experience {i}"),
            embedding: Some(make_embedding(i)),
            ..Default::default()
        })
        .unwrap();
    }

    (db, cid, dir)
}

/// Scaling benchmark: search_similar at different DB sizes.
///
/// Performance targets (SRS NFR-004):
/// - k=20 search should be < 50ms at 100K experiences
fn bench_search_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("search_similar_scaling");
    group.measurement_time(Duration::from_secs(10));

    for &size in &[1_000usize, 10_000, 100_000] {
        // Reduce sample count for larger sizes to keep total time reasonable
        if size >= 100_000 {
            group.sample_size(10);
        } else if size >= 10_000 {
            group.sample_size(20);
        }

        let (db, cid, _dir) = setup_db_with_n(size);
        let query = make_embedding(size as u64 + 1);

        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, _| {
            b.iter(|| db.search_similar(cid, &query, 20).unwrap());
        });
    }
    group.finish();
}

/// Scaling benchmark: record_experience throughput at different DB sizes.
///
/// Measures write latency as the database grows.
/// Performance target (SRS NFR-002): < 10ms at 100K experiences
fn bench_write_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("record_experience_scaling");
    group.measurement_time(Duration::from_secs(10));

    for &size in &[1_000usize, 10_000, 100_000] {
        if size >= 100_000 {
            group.sample_size(10);
        } else if size >= 10_000 {
            group.sample_size(20);
        }

        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &size| {
            b.iter_custom(|iters| {
                let (db, cid, _dir) = setup_db_with_n(size);
                let mut total = std::time::Duration::ZERO;

                for i in 0..iters {
                    let seed = size as u64 + i + 1;
                    let start = std::time::Instant::now();
                    db.record_experience(NewExperience {
                        collective_id: cid,
                        content: format!("Write {seed}"),
                        embedding: Some(make_embedding(seed)),
                        ..Default::default()
                    })
                    .unwrap();
                    total += start.elapsed();
                }

                total
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_search_scaling, bench_write_scaling);
criterion_main!(benches);
