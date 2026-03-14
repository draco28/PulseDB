//! Fuzz target: create_collective
//!
//! Feeds arbitrary bytes as collective names. Verifies PulseDB never panics
//! on invalid names — validation should return errors, not crash.

#![no_main]

use libfuzzer_sys::fuzz_target;
use pulsedb::{Config, PulseDB};
use tempfile::tempdir;

fuzz_target!(|data: &[u8]| {
    let dir = tempdir().unwrap();
    let db = PulseDB::open(dir.path().join("fuzz.db"), Config::default()).unwrap();

    // Convert fuzz bytes to a string (lossy — may contain replacement chars)
    let name = String::from_utf8_lossy(data);

    // Should never panic — empty names, huge names, unicode names should all
    // return a Result, not crash
    let _ = db.create_collective(&name);
});
