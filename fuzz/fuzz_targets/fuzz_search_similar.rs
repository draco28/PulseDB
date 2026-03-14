//! Fuzz target: search_similar
//!
//! Pre-populates a database with 5 fixed experiences, then searches with
//! a fuzz-derived query vector and k value. Asserts no panics.

#![no_main]

use libfuzzer_sys::fuzz_target;
use pulsedb::{Config, NewExperience, PulseDB};
use tempfile::tempdir;

/// Embedding dimension must match Config::default() (D384).
const DIM: usize = 384;

/// Generates a deterministic embedding from a seed (same as test helpers).
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

fuzz_target!(|data: &[u8]| {
    if data.len() < 2 {
        return;
    }

    let dir = tempdir().unwrap();
    let db = PulseDB::open(dir.path().join("fuzz.db"), Config::default()).unwrap();
    let cid = db.create_collective("fuzz").unwrap();

    // Pre-populate with 5 fixed experiences for HNSW graph connectivity
    for seed in 0u64..5 {
        let _ = db.record_experience(NewExperience {
            collective_id: cid,
            content: format!("Seed experience {seed}"),
            embedding: Some(make_embedding(seed)),
            ..Default::default()
        });
    }

    // Derive query vector from fuzz data
    let query: Vec<f32> = data
        .iter()
        .cycle()
        .take(DIM)
        .map(|&b| (b as f32) / 255.0 - 0.5)
        .collect();

    // Derive k from first two bytes (clamped to reasonable range)
    let k = (u16::from_le_bytes([data[0], data[1]]) as usize).max(1);

    // Should never panic — may return Ok or Err (e.g., k > 1000 validation)
    let _ = db.search_similar(cid, &query, k);
});
