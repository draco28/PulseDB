//! Benchmarks for realistic PulseDB workload patterns.
//!
//! Run with: `cargo bench -- workloads`
//!
//! Simulates mixed read/write workloads typical of agentic AI systems:
//! - 80% reads (search_similar) / 20% writes (record_experience)

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use pulsedb::{CollectiveId, Config, NewExperience, PulseDB};
use std::cell::Cell;
use tempfile::tempdir;

/// Default embedding dimension (D384).
const DIM: usize = 384;

/// Number of experiences to pre-populate.
const SEED_COUNT: usize = 1_000;

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

/// Sets up a database pre-populated with SEED_COUNT experiences.
fn setup_populated_db() -> (PulseDB, CollectiveId, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let db = PulseDB::open(dir.path().join("bench.db"), Config::default()).unwrap();
    let cid = db.create_collective("bench").unwrap();

    for i in 0..SEED_COUNT as u64 {
        db.record_experience(NewExperience {
            collective_id: cid,
            content: format!("Setup experience {i}"),
            embedding: Some(make_embedding(i)),
            ..Default::default()
        })
        .unwrap();
    }

    (db, cid, dir)
}

/// Mixed workload: 80% reads (search_similar) / 20% writes (record_experience).
///
/// Each iteration performs 10 operations: 8 searches + 2 writes.
/// This simulates a typical agentic AI workload where agents mostly query
/// for context but occasionally record new experiences.
fn bench_mixed_80read_20write(c: &mut Criterion) {
    let (db, cid, _dir) = setup_populated_db();
    let query = make_embedding(999_999);
    let write_seed = Cell::new(SEED_COUNT as u64);

    c.bench_function("mixed_80read_20write", |b| {
        b.iter(|| {
            for i in 0..10u32 {
                if i % 5 == 0 {
                    // 20% writes (iterations 0 and 5)
                    let seed = write_seed.get();
                    write_seed.set(seed + 1);
                    let _ = black_box(db.record_experience(NewExperience {
                        collective_id: cid,
                        content: format!("Write {seed}"),
                        embedding: Some(make_embedding(seed)),
                        ..Default::default()
                    }));
                } else {
                    // 80% reads (iterations 1,2,3,4,6,7,8,9)
                    let _ = black_box(db.search_similar(cid, &query, 20));
                }
            }
        });
    });
}

criterion_group!(benches, bench_mixed_80read_20write);
criterion_main!(benches);
