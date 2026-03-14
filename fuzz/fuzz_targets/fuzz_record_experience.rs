//! Fuzz target: record_experience
//!
//! Feeds arbitrary bytes as experience content and derived embedding values.
//! Asserts that PulseDB never panics, regardless of input — errors are fine.

#![no_main]

use libfuzzer_sys::fuzz_target;
use pulsedb::{Config, NewExperience, PulseDB};
use tempfile::tempdir;

/// Embedding dimension must match Config::default() (D384).
const DIM: usize = 384;

fuzz_target!(|data: &[u8]| {
    // Need at least 2 bytes for importance + confidence
    if data.is_empty() {
        return;
    }

    let dir = tempdir().unwrap();
    let db = PulseDB::open(dir.path().join("fuzz.db"), Config::default()).unwrap();
    let cid = db.create_collective("fuzz").unwrap();

    // Derive content from fuzz data (lossy UTF-8 conversion)
    let content = String::from_utf8_lossy(data).to_string();

    // Build embedding by cycling through fuzz bytes and normalizing to [-0.5, 0.5]
    let embedding: Vec<f32> = data
        .iter()
        .cycle()
        .take(DIM)
        .map(|&b| (b as f32) / 255.0 - 0.5)
        .collect();

    // Derive importance and confidence from first bytes
    let importance = data[0] as f32 / 255.0;
    let confidence = if data.len() > 1 {
        data[1] as f32 / 255.0
    } else {
        0.5
    };

    // Should never panic — may return Ok or Err (e.g., empty content after trim)
    let _ = db.record_experience(NewExperience {
        collective_id: cid,
        content,
        embedding: Some(embedding),
        importance,
        confidence,
        ..Default::default()
    });
});
