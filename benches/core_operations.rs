//! Benchmarks for PulseDB core operations.
//!
//! Run with: `cargo bench -- core_operations`
//!
//! Performance targets (SRS NFR-002/003/004):
//! - `record_experience` < 10ms
//! - `search_similar` (k=20) < 50ms
//! - `get_context_candidates` < 100ms
//! - `get_experience` — baseline measurement

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use pulsedb::{CollectiveId, Config, ContextRequest, NewExperience, PulseDB};
use tempfile::tempdir;

/// Default embedding dimension (D384, matches Config::default()).
const DIM: usize = 384;

/// Number of experiences to pre-populate for benchmarks.
const SEED_COUNT: usize = 1_000;

/// Generates a deterministic embedding from a seed.
///
/// Uses a hash-based pseudo-random generator to produce well-separated vectors
/// in the 384-dimensional space. Same algorithm as test helpers.
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
///
/// Returns (db, collective_id, first_experience_id, tempdir).
/// The tempdir must be kept alive for the DB's lifetime.
fn setup_populated_db() -> (PulseDB, CollectiveId, pulsedb::ExperienceId, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let db = PulseDB::open(dir.path().join("bench.db"), Config::default()).unwrap();
    let cid = db.create_collective("bench").unwrap();

    let mut first_id = None;
    for i in 0..SEED_COUNT as u64 {
        let id = db
            .record_experience(NewExperience {
                collective_id: cid,
                content: format!("Benchmark experience {i}"),
                embedding: Some(make_embedding(i)),
                ..Default::default()
            })
            .unwrap();
        if first_id.is_none() {
            first_id = Some(id);
        }
    }

    (db, cid, first_id.unwrap(), dir)
}

/// Benchmark: record_experience (target < 10ms).
///
/// Uses `iter_custom` because each write mutates the DB. Opens a fresh
/// 1K-populated DB for each iteration batch to avoid measurement drift.
fn bench_record_experience(c: &mut Criterion) {
    c.bench_function("record_experience", |b| {
        b.iter_custom(|iters| {
            let dir = tempdir().unwrap();
            let db = PulseDB::open(dir.path().join("bench.db"), Config::default()).unwrap();
            let cid = db.create_collective("bench").unwrap();

            // Pre-populate with 1K experiences
            for i in 0..SEED_COUNT as u64 {
                db.record_experience(NewExperience {
                    collective_id: cid,
                    content: format!("Setup {i}"),
                    embedding: Some(make_embedding(i)),
                    ..Default::default()
                })
                .unwrap();
            }

            // Time only the new writes
            let mut total = std::time::Duration::ZERO;
            for i in 0..iters {
                let seed = SEED_COUNT as u64 + i;
                let start = std::time::Instant::now();
                let _ = black_box(db.record_experience(NewExperience {
                    collective_id: cid,
                    content: format!("Bench write {seed}"),
                    embedding: Some(make_embedding(seed)),
                    ..Default::default()
                }));
                total += start.elapsed();
            }

            total
        });
    });
}

/// Benchmark: search_similar k=20 (target < 50ms).
///
/// DB is read-only during measurement, so setup once and use `b.iter()`.
fn bench_search_similar(c: &mut Criterion) {
    let (db, cid, _, _dir) = setup_populated_db();
    let query = make_embedding(999_999); // Seed not in DB

    c.bench_function("search_similar_k20", |b| {
        b.iter(|| black_box(db.search_similar(cid, &query, 20).unwrap()));
    });
}

/// Benchmark: get_context_candidates (target < 100ms).
///
/// Exercises the full orchestration: similar + recent + insights + relations + agents.
fn bench_get_context_candidates(c: &mut Criterion) {
    let (db, cid, _, _dir) = setup_populated_db();
    let query = make_embedding(999_999);

    c.bench_function("get_context_candidates", |b| {
        b.iter(|| {
            black_box(
                db.get_context_candidates(ContextRequest {
                    collective_id: cid,
                    query_embedding: query.clone(),
                    max_similar: 20,
                    ..ContextRequest::default()
                })
                .unwrap(),
            )
        });
    });
}

/// Benchmark: get_experience by ID (baseline measurement).
///
/// Point lookup — should be sub-millisecond since it's a single redb key read.
fn bench_get_experience(c: &mut Criterion) {
    let (db, _, first_id, _dir) = setup_populated_db();

    c.bench_function("get_experience_by_id", |b| {
        b.iter(|| black_box(db.get_experience(first_id).unwrap()));
    });
}

criterion_group!(
    benches,
    bench_record_experience,
    bench_search_similar,
    bench_get_context_candidates,
    bench_get_experience
);
criterion_main!(benches);
