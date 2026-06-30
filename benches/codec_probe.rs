//! VS-4.0.3 work-1.02 — Scale + codec characterization probe.
//!
//! Run with:
//!   - Part A (codec size/speed delta, fast):  `cargo bench --bench codec_probe`
//!   - Part B (single-txn viability, scaled):  `CODEC_PROBE_VIABILITY=1 cargo bench --bench codec_probe -- --nocapture`
//!     optional knobs: `CODEC_PROBE_NS=10000,50000,100000,250000` (comma-sep N ladder)
//!
//! ===========================================================================
//! WHY THIS PROBE EXISTS (grill-me gate-1 finding)
//! ===========================================================================
//! 1.04 re-encodes every serde-blob value (bincode -> postcard, ADR-006) inside
//! `open_existing`'s SINGLE redb write transaction. A redb write-txn buffers all
//! dirty pages until commit, and the Sprint-3.5 perf fixture was ~4GB / 1M rows.
//! If a whole-store re-encode can't commit in one txn at that scale, "atomic
//! single txn" doesn't just get slow — it never finishes (OOM). This probe
//! MEASURES the picture and produces:
//!   (A) the postcard-vs-bincode size/speed delta per stored type (ADR-006 mit.8),
//!       with the 384-d `Vec<f32>` embedding case called out separately, and
//!   (B) the peak-memory cost coefficient (peak dirty-buffer bytes per stored
//!       byte) + a config-first single-txn-vs-phased decision rule for 1.04.
//!
//! ===========================================================================
//! CRITICAL STORAGE FACT (drives the coefficient; verified from src/storage)
//! ===========================================================================
//! Embeddings are NOT serde values. `save_experience` stores each embedding as
//! RAW little-endian f32 bytes via `f32_slice_to_bytes` into EMBEDDINGS_TABLE
//! (see src/storage/redb.rs). They are codec-INDEPENDENT: a bincode->postcard
//! re-encode does NOT touch them. The serde-blob value classes that 1.04
//! actually re-encodes are the small per-record structs:
//!   metadata, collectives, decay_configs, experiences (embedding #[serde(skip)]),
//!   relations, insights, sync_cursors, watch_events.
//! At 384-d, the embedding (1,536 raw bytes/row) DOMINATES per-row store size;
//! the re-encoded serde blob is a small fraction. => the single-txn dirty set is
//! a FRACTION of total store size, because the giant embedding table is not
//! rewritten. This is the key input that makes single-txn far safer than the
//! naive "peak ~= store size" worst case the grill-me review feared.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use serde::Serialize;

use pulsedb::{
    AgentId, Collective, CollectiveId, Config, DerivedInsight, Experience, ExperienceId,
    ExperienceType, InsightId, InsightType, InstanceId, NewExperience, PulseDB, Timestamp,
};

/// Embedding dimension (D384 — Config::default()).
const DIM: usize = 384;

// ===========================================================================
// Representative-record builders (deterministic; mirror seed_representative_*)
// ===========================================================================

/// Deterministic 384-d embedding with a non-uniform pattern.
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

/// A representative `Experience` — multi-entry `applications` BTreeMap,
/// non-trivial `experience_type`, multi-tag `domain` + `related_files`.
/// NOTE: `embedding` is `#[serde(skip)]`, so it is excluded from the blob.
fn representative_experience() -> Experience {
    let ts = Timestamp::from_millis(1_700_000_123_456);
    let mut applications: BTreeMap<InstanceId, u32> = BTreeMap::new();
    applications.insert(InstanceId::from_bytes(*b"INSTANCE_ALPHA__"), 3);
    applications.insert(InstanceId::from_bytes(*b"INSTANCE_BETA___"), 5);
    applications.insert(InstanceId::from_bytes(*b"INSTANCE_GAMMA__"), 11);
    Experience {
        id: ExperienceId::from_bytes(*b"REPRESENTATIVEXP"),
        collective_id: CollectiveId::from_bytes(*b"REPRESENTATIVE__"),
        content: "representative v0.5.1 experience carried across the redb v2->v3 upgrade; \
                  multi-sentence content to approximate a realistic stored record."
            .into(),
        embedding: make_embedding(7), // serde-skipped; present for realism only
        experience_type: ExperienceType::ErrorPattern {
            signature: "E0308 mismatched types in async closure".into(),
            fix: "annotate the return type or box the future".into(),
            prevention: "prefer explicit Future bounds on stored closures".into(),
        },
        importance: 0.73,
        confidence: 0.91,
        applications,
        domain: vec!["migration".into(), "redb".into(), "storage-format".into()],
        related_files: vec!["src/storage/redb.rs".into(), "src/storage/schema.rs".into()],
        source_agent: AgentId::new("representative-agent"),
        source_task: None,
        timestamp: ts,
        last_reinforced: ts,
        archived: false,
    }
}

/// A representative `Collective`.
fn representative_collective() -> Collective {
    let ts = Timestamp::from_millis(1_700_000_000_000);
    Collective {
        id: CollectiveId::from_bytes(*b"REPRESENTATIVE__"),
        name: "representative-v2-collective".into(),
        owner_id: Some("owner-42".into()),
        embedding_dimension: 384,
        created_at: ts,
        updated_at: ts,
    }
}

/// A representative `DerivedInsight` — embedding stored INLINE (not skipped),
/// so this blob carries a full 384-d `Vec<f32>` through serde.
fn representative_insight() -> DerivedInsight {
    let ts = Timestamp::from_millis(1_700_000_222_222);
    DerivedInsight {
        id: InsightId::from_bytes(*b"REPRESENTINSIGHT"),
        collective_id: CollectiveId::from_bytes(*b"REPRESENTATIVE__"),
        content: "Error-handling patterns across this collective converge on early-return; \
                  inline-embedding insight record for the codec probe."
            .into(),
        embedding: make_embedding(99),
        source_experience_ids: vec![
            ExperienceId::from_bytes(*b"SOURCEEXPONE____"),
            ExperienceId::from_bytes(*b"SOURCEEXPTWO____"),
            ExperienceId::from_bytes(*b"SOURCEEXPTHREE__"),
        ],
        insight_type: InsightType::Pattern,
        confidence: 0.85,
        domain: vec!["rust".into(), "error-handling".into()],
        created_at: ts,
        updated_at: ts,
    }
}

// ===========================================================================
// Part A — codec size/speed delta (bincode vs postcard)
// ===========================================================================

/// Encoded size (bytes) for a value under both codecs.
fn sizes<T: Serialize>(v: &T) -> (usize, usize) {
    let b = bincode::serialize(v).unwrap().len();
    let p = postcard::to_allocvec(v).unwrap().len();
    (b, p)
}

fn pct(from: usize, to: usize) -> f64 {
    if from == 0 {
        return 0.0;
    }
    (to as f64 - from as f64) / from as f64 * 100.0
}

/// Print the size table once (eprintln so it shows under --nocapture and in the
/// stderr log the orchestrator reads).
fn print_size_table() {
    let exp = representative_experience();
    let col = representative_collective();
    let ins = representative_insight();
    let emb = make_embedding(1); // standalone 384-d Vec<f32> (ADR varint risk case)

    let (eb, ep) = sizes(&exp);
    let (cb, cp) = sizes(&col);
    let (ib, ip) = sizes(&ins);
    let (vb, vp) = sizes(&emb);
    // Raw LE bytes is how production actually stores the embedding (NOT serde).
    let raw_emb = emb.len() * 4;

    eprintln!("=== CODEC SIZE DELTA (bincode vs postcard) ===");
    eprintln!(
        "{:<34} {:>10} {:>10} {:>10}",
        "type", "bincode", "postcard", "delta%"
    );
    eprintln!(
        "{:<34} {:>10} {:>10} {:>9.1}%",
        "Experience (embedding serde-skipped)", eb, ep, pct(eb, ep)
    );
    eprintln!(
        "{:<34} {:>10} {:>10} {:>9.1}%",
        "Collective", cb, cp, pct(cb, cp)
    );
    eprintln!(
        "{:<34} {:>10} {:>10} {:>9.1}%",
        "DerivedInsight (inline 384-d emb)", ib, ip, pct(ib, ip)
    );
    eprintln!(
        "{:<34} {:>10} {:>10} {:>9.1}%",
        "Vec<f32> 384-d (serde, ADR risk)", vb, vp, pct(vb, vp)
    );
    eprintln!(
        "{:<34} {:>10} {:>10} {:>9}",
        "Vec<f32> 384-d (RAW LE = production)", raw_emb, raw_emb, "n/a"
    );
    eprintln!("(note: production stores embeddings as RAW LE f32 bytes, codec-independent — the");
    eprintln!(" serde Vec<f32> row above is the ADR-006 'no varint compression' case for reference)");
    eprintln!("==============================================");
}

fn codec_size(c: &mut Criterion) {
    print_size_table();
    // Register a trivial criterion bench so `cargo bench` selection includes us.
    let exp = representative_experience();
    c.bench_function("codec_size_marker", |b| {
        b.iter(|| black_box(bincode::serialize(&exp).unwrap().len()))
    });
}

fn codec_speed(c: &mut Criterion) {
    let exp = representative_experience();
    let ins = representative_insight();
    let emb = make_embedding(3);

    let exp_bin = bincode::serialize(&exp).unwrap();
    let exp_post = postcard::to_allocvec(&exp).unwrap();
    let ins_bin = bincode::serialize(&ins).unwrap();
    let ins_post = postcard::to_allocvec(&ins).unwrap();
    let emb_bin = bincode::serialize(&emb).unwrap();
    let emb_post = postcard::to_allocvec(&emb).unwrap();

    let mut g = c.benchmark_group("codec_speed");

    g.bench_function("experience/ser/bincode", |b| {
        b.iter(|| black_box(bincode::serialize(&exp).unwrap()))
    });
    g.bench_function("experience/ser/postcard", |b| {
        b.iter(|| black_box(postcard::to_allocvec(&exp).unwrap()))
    });
    g.bench_function("experience/de/bincode", |b| {
        b.iter(|| black_box(bincode::deserialize::<Experience>(&exp_bin).unwrap()))
    });
    g.bench_function("experience/de/postcard", |b| {
        b.iter(|| black_box(postcard::from_bytes::<Experience>(&exp_post).unwrap()))
    });

    g.bench_function("insight/ser/bincode", |b| {
        b.iter(|| black_box(bincode::serialize(&ins).unwrap()))
    });
    g.bench_function("insight/ser/postcard", |b| {
        b.iter(|| black_box(postcard::to_allocvec(&ins).unwrap()))
    });
    g.bench_function("insight/de/bincode", |b| {
        b.iter(|| black_box(bincode::deserialize::<DerivedInsight>(&ins_bin).unwrap()))
    });
    g.bench_function("insight/de/postcard", |b| {
        b.iter(|| black_box(postcard::from_bytes::<DerivedInsight>(&ins_post).unwrap()))
    });

    g.bench_function("embedding_vecf32/ser/bincode", |b| {
        b.iter(|| black_box(bincode::serialize(&emb).unwrap()))
    });
    g.bench_function("embedding_vecf32/ser/postcard", |b| {
        b.iter(|| black_box(postcard::to_allocvec(&emb).unwrap()))
    });
    g.bench_function("embedding_vecf32/de/bincode", |b| {
        b.iter(|| black_box(bincode::deserialize::<Vec<f32>>(&emb_bin).unwrap()))
    });
    g.bench_function("embedding_vecf32/de/postcard", |b| {
        b.iter(|| black_box(postcard::from_bytes::<Vec<f32>>(&emb_post).unwrap()))
    });

    g.finish();
}

// ===========================================================================
// Part B — single-txn viability: peak-memory coefficient (scaled, env-gated)
// ===========================================================================

/// Peak resident-set size in bytes (high-water mark) via getrusage(RUSAGE_SELF).
/// macOS reports ru_maxrss in BYTES; Linux reports it in KIBIBYTES.
fn peak_rss_bytes() -> u64 {
    #[repr(C)]
    #[derive(Default)]
    struct Timeval {
        tv_sec: i64,
        tv_usec: i64,
    }
    // Only ru_maxrss is read; the layout up to it is identical on macOS/Linux
    // (two timevals then a c_long). Over-allocate the tail to be safe.
    #[repr(C)]
    struct Rusage {
        ru_utime: Timeval,
        ru_stime: Timeval,
        ru_maxrss: i64,
        _tail: [i64; 16],
    }
    extern "C" {
        fn getrusage(who: i32, usage: *mut Rusage) -> i32;
    }
    const RUSAGE_SELF: i32 = 0;
    let mut usage = Rusage {
        ru_utime: Timeval::default(),
        ru_stime: Timeval::default(),
        ru_maxrss: 0,
        _tail: [0; 16],
    };
    let rc = unsafe { getrusage(RUSAGE_SELF, &mut usage as *mut Rusage) };
    if rc != 0 {
        return 0;
    }
    let maxrss = usage.ru_maxrss.max(0) as u64;
    if cfg!(target_os = "macos") {
        maxrss // already bytes
    } else {
        maxrss * 1024 // KiB -> bytes
    }
}

/// Total bytes-on-disk of the redb store file (proxy for store size).
fn file_size_bytes(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// Seed `n` experiences via the public PulseDB API (the exact production write
/// path, incl. HNSW). Faithful but SLOW (HNSW build dominates) — used only to
/// validate the direct seeder agrees, at small N. Returns the .db path.
#[allow(dead_code)]
fn seed_store_via_api(dir: &Path, n: usize) -> std::path::PathBuf {
    let db_path = dir.join(format!("probe-api-{n}.db"));
    let db = PulseDB::open(&db_path, Config::default()).unwrap();
    let cid = db.create_collective("probe").unwrap();
    for i in 0..n as u64 {
        db.record_experience(NewExperience {
            collective_id: cid,
            content: format!("Experience {i} — representative content for the scale probe"),
            importance: 0.35 + ((i % 100) as f32 / 200.0),
            embedding: Some(make_embedding(i)),
            ..Default::default()
        })
        .unwrap();
    }
    db.close().unwrap();
    db_path
}

/// Fast direct-redb seeder: writes the SAME byte layout production writes
/// (bincode Experience blob with embedding `#[serde(skip)]`, raw-LE embedding in
/// EMBEDDINGS_TABLE, the two index multimaps + metadata + a collective) but
/// SKIPS the HNSW build, so 250k rows is tractable. The re-encode cost + store
/// composition this produces are identical to production for the serde-blob
/// tables 1.04 touches. Returns the .db path.
fn seed_store_direct(dir: &Path, n: usize) -> std::path::PathBuf {
    use redb::{Database, MultimapTableDefinition, TableDefinition};

    const METADATA: TableDefinition<&str, &[u8]> = TableDefinition::new("metadata");
    const COLLECTIVES: TableDefinition<&[u8; 16], &[u8]> = TableDefinition::new("collectives");
    const EXPERIENCES: TableDefinition<&[u8; 16], &[u8]> = TableDefinition::new("experiences");
    const EMBEDDINGS: TableDefinition<&[u8; 16], &[u8]> = TableDefinition::new("embeddings");
    const BY_COLLECTIVE: MultimapTableDefinition<&[u8; 16], &[u8; 24]> =
        MultimapTableDefinition::new("experiences_by_collective");
    const BY_TYPE: MultimapTableDefinition<&[u8; 17], &[u8; 16]> =
        MultimapTableDefinition::new("experiences_by_type");

    let db_path = dir.join(format!("probe-{n}.db"));
    let db = Database::builder().create(&db_path).unwrap();
    let cid_bytes = *b"PROBECOLLECTIVE_";

    // Collective + metadata (small, one-shot).
    let collective = Collective {
        id: CollectiveId::from_bytes(cid_bytes),
        name: "probe".into(),
        owner_id: None,
        embedding_dimension: 384,
        created_at: Timestamp::from_millis(1_700_000_000_000),
        updated_at: Timestamp::from_millis(1_700_000_000_000),
    };

    // Chunk the seed across several write-txns so the SEED phase itself does not
    // buffer all N dirty pages (we only want the RE-ENCODE txn to be single).
    let chunk = 25_000usize;
    let mut i = 0u64;
    {
        let wt = db.begin_write().unwrap();
        {
            let mut m = wt.open_table(METADATA).unwrap();
            // marker not needed; store size is what matters. Write a small blob.
            m.insert("metadata", bincode::serialize(&collective).unwrap().as_slice())
                .unwrap();
        }
        {
            let mut c = wt.open_table(COLLECTIVES).unwrap();
            c.insert(&cid_bytes, bincode::serialize(&collective).unwrap().as_slice())
                .unwrap();
        }
        wt.commit().unwrap();
    }
    while (i as usize) < n {
        let end = ((i as usize) + chunk).min(n) as u64;
        let wt = db.begin_write().unwrap();
        {
            let mut exp_t = wt.open_table(EXPERIENCES).unwrap();
            let mut emb_t = wt.open_table(EMBEDDINGS).unwrap();
            let mut byc = wt.open_multimap_table(BY_COLLECTIVE).unwrap();
            let mut byt = wt.open_multimap_table(BY_TYPE).unwrap();
            for j in i..end {
                let mut id = [0u8; 16];
                id[..8].copy_from_slice(&j.to_le_bytes());
                id[8..].copy_from_slice(&(j ^ 0xA5A5_A5A5_A5A5_A5A5).to_le_bytes());
                let ts = Timestamp::from_millis(1_700_000_000_000 + j as i64);
                let exp = Experience {
                    id: ExperienceId::from_bytes(id),
                    collective_id: CollectiveId::from_bytes(cid_bytes),
                    content: format!("Experience {j} — representative content for the scale probe"),
                    embedding: Vec::new(), // serde-skipped anyway
                    experience_type: ExperienceType::Generic { category: None },
                    importance: 0.35 + ((j % 100) as f32 / 200.0),
                    confidence: 0.5,
                    applications: BTreeMap::new(),
                    domain: vec!["probe".into()],
                    related_files: Vec::new(),
                    source_agent: AgentId::new("anonymous"),
                    source_task: None,
                    timestamp: ts,
                    last_reinforced: ts,
                    archived: false,
                };
                let exp_bytes = bincode::serialize(&exp).unwrap();
                exp_t.insert(&id, exp_bytes.as_slice()).unwrap();

                // Raw LE f32 embedding — exactly production's f32_slice_to_bytes.
                let emb = make_embedding(j);
                let mut emb_bytes = Vec::with_capacity(emb.len() * 4);
                for v in &emb {
                    emb_bytes.extend_from_slice(&v.to_le_bytes());
                }
                emb_t.insert(&id, emb_bytes.as_slice()).unwrap();

                let mut byc_val = [0u8; 24];
                byc_val[..8].copy_from_slice(&ts.to_be_bytes());
                byc_val[8..24].copy_from_slice(&id);
                byc.insert(&cid_bytes, &byc_val).unwrap();

                let mut type_key = [0u8; 17];
                type_key[..16].copy_from_slice(&cid_bytes);
                type_key[16] = 8; // Generic tag (last variant); value is the id
                byt.insert(&type_key, &id).unwrap();
            }
        }
        wt.commit().unwrap();
        i = end;
    }
    drop(db);
    db_path
}

/// Model 1.04's whole-store serde-blob re-encode inside ONE redb write-txn,
/// measuring commit-time. This re-encodes the EXPERIENCES table genuinely
/// (bincode-decode -> postcard-encode, the dominant serde table) and byte-
/// rewrites the small remaining serde-blob tables, while leaving the raw-byte
/// EMBEDDINGS_TABLE untouched (faithful: embeddings are NOT serde). The whole
/// re-encode runs in ONE write-txn so redb buffers every dirty page until the
/// single commit — exactly the OOM-risk shape this probe characterizes.
/// Returns (commit_ms, dirty_bytes_written) where dirty_bytes is the total
/// re-encoded value+key bytes (the lower bound on the buffered dirty set).
fn single_txn_reencode(db_path: &Path) -> (f64, u64) {
    use redb::{Database, ReadableTable, TableDefinition};

    const METADATA: TableDefinition<&str, &[u8]> = TableDefinition::new("metadata");
    const COLLECTIVES: TableDefinition<&[u8; 16], &[u8]> = TableDefinition::new("collectives");
    const EXPERIENCES: TableDefinition<&[u8; 16], &[u8]> = TableDefinition::new("experiences");

    let db = Database::open(db_path).unwrap();

    // Pre-read the experiences snapshot OUTSIDE the timed/committed txn boundary?
    // No — 1.04 reads + re-encodes inside the SAME write-txn it commits. We open
    // the write-txn, iterate (read), re-encode, and re-insert, then commit once.
    let write_txn = db.begin_write().unwrap();
    let mut dirty: u64 = 0;

    // EXPERIENCES — the dominant serde table. Genuine bincode->postcard re-encode.
    let exp_pairs: Vec<([u8; 16], Vec<u8>)> = {
        let t = write_txn.open_table(EXPERIENCES).unwrap();
        t.iter()
            .unwrap()
            .filter_map(|r| r.ok())
            .map(|(k, v)| (*k.value(), v.value().to_vec()))
            .collect()
    };
    {
        let mut t = write_txn.open_table(EXPERIENCES).unwrap();
        for (k, v) in &exp_pairs {
            // bincode -> typed -> postcard, exactly 1.04's value re-encode.
            let exp: Experience = bincode::deserialize(v).unwrap();
            let re = postcard::to_allocvec(&exp).unwrap();
            dirty += (k.len() + re.len()) as u64;
            t.insert(k, re.as_slice()).unwrap();
        }
    }

    // COLLECTIVES + METADATA — small; genuine re-encode for collectives.
    let col_pairs: Vec<([u8; 16], Vec<u8>)> = {
        match write_txn.open_table(COLLECTIVES) {
            Ok(t) => t
                .iter()
                .unwrap()
                .filter_map(|r| r.ok())
                .map(|(k, v)| (*k.value(), v.value().to_vec()))
                .collect(),
            Err(_) => Vec::new(),
        }
    };
    {
        let mut t = write_txn.open_table(COLLECTIVES).unwrap();
        for (k, v) in &col_pairs {
            let col: Collective = bincode::deserialize(v).unwrap();
            let re = postcard::to_allocvec(&col).unwrap();
            dirty += (k.len() + re.len()) as u64;
            t.insert(k, re.as_slice()).unwrap();
        }
    }
    {
        // metadata blob: byte-rewrite (its concrete type is private; size is tiny).
        let meta_v: Option<Vec<u8>> = write_txn
            .open_table(METADATA)
            .ok()
            .and_then(|t| t.get("metadata").ok().flatten().map(|v| v.value().to_vec()));
        if let Some(v) = meta_v {
            let mut t = write_txn.open_table(METADATA).unwrap();
            dirty += ("metadata".len() + v.len()) as u64;
            t.insert("metadata", v.as_slice()).unwrap();
        }
    }

    // Commit the single transaction — redb flushes the buffered dirty pages here.
    let t0 = Instant::now();
    write_txn.commit().unwrap();
    let commit_ms = t0.elapsed().as_secs_f64() * 1000.0;
    (commit_ms, dirty)
}

fn viability_probe() {
    let ns: Vec<usize> = std::env::var("CODEC_PROBE_NS")
        .ok()
        .map(|s| {
            s.split(',')
                .filter_map(|x| x.trim().parse::<usize>().ok())
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| vec![10_000, 50_000, 100_000, 250_000]);

    eprintln!("=== SINGLE-TXN VIABILITY PROBE (Part B) ===");
    eprintln!("host: peak RSS via getrusage(RUSAGE_SELF) high-water; re-encode = bincode->postcard");
    eprintln!("seed: direct-redb (production byte layout, HNSW skipped); re-encode in ONE write-txn");
    eprintln!(
        "{:>9} {:>12} {:>14} {:>12} {:>14} {:>10}",
        "N", "store_MB", "reencoded_MB", "commit_ms", "rss_delta_MB", "COEFF"
    );

    let dir = tempfile::tempdir().unwrap();
    // (N, store_bytes, dirty_bytes, commit_ms, rss_delta_bytes, rss_peak_bytes)
    let mut rows: Vec<(usize, u64, u64, f64, u64, u64)> = Vec::new();

    for &n in &ns {
        let db_path = seed_store_direct(dir.path(), n);
        let store = file_size_bytes(&db_path);

        // High-water RSS AFTER seed (the seed's transient pages have peaked + been
        // dropped). The re-encode txn's incremental RSS growth above this floor is
        // the per-txn dirty-buffer cost we attribute to the single-txn re-encode.
        let rss_floor = peak_rss_bytes();
        let (commit_ms, dirty) = single_txn_reencode(&db_path);
        let rss_peak = peak_rss_bytes();
        let rss_delta = rss_peak.saturating_sub(rss_floor);

        let store_mb = store / (1024 * 1024);
        let dirty_mb = dirty / (1024 * 1024);
        let dps = if store > 0 {
            dirty as f64 / store as f64
        } else {
            0.0
        };
        eprintln!(
            "{:>9} {:>12} {:>14} {:>12.1} {:>14} {:>10.4}",
            n,
            store_mb,
            dirty_mb,
            commit_ms,
            rss_delta / (1024 * 1024),
            dps
        );
        rows.push((n, store, dirty, commit_ms, rss_delta, rss_peak));

        let _ = std::fs::remove_file(&db_path);
    }

    // Fit the coefficient from the LARGEST N (post-cliff regime if any appeared).
    if let Some(&(n, store, dirty, commit_ms, rss_delta, rss_peak)) = rows.last() {
        let dirty_per_store = if store > 0 {
            dirty as f64 / store as f64
        } else {
            0.0
        };
        let dirty_per_row = dirty as f64 / n.max(1) as f64;
        let rss_delta_per_store = if store > 0 {
            rss_delta as f64 / store as f64
        } else {
            0.0
        };
        eprintln!("--- COEFFICIENT FIT (largest N = {n}) ---");
        eprintln!(
            "  re-encoded serde dirty set = {} bytes ({:.1} MB)",
            dirty,
            dirty as f64 / (1024.0 * 1024.0)
        );
        eprintln!("  total store size           = {store} bytes ({} MB)", store / (1024 * 1024));
        eprintln!(
            "  COEFF (dirty / store)      = {dirty_per_store:.4}  <-- peak-dirty-buffer bytes per stored byte"
        );
        eprintln!("  dirty-per-row              = {dirty_per_row:.1} bytes/row (serde re-encode set)");
        eprintln!(
            "  re-encode txn RSS delta    = {} MB  (RSS growth attributable to the single txn)",
            rss_delta / (1024 * 1024)
        );
        eprintln!(
            "  re-encode RSS / store      = {rss_delta_per_store:.4}  (incremental, above post-seed floor)"
        );
        eprintln!("  commit_ms (largest N)      = {commit_ms:.1} ms; process peak RSS = {} MB", rss_peak / (1024 * 1024));
        eprintln!("  >> single-txn peak memory ~= base_RSS + COEFF * store_size  (COEFF measured above)");
        eprintln!("==============================================");
    }
}

fn viability(c: &mut Criterion) {
    if std::env::var("CODEC_PROBE_VIABILITY").ok().as_deref() == Some("1") {
        viability_probe();
    } else {
        eprintln!(
            "codec_probe Part B (single-txn viability) SKIPPED — set CODEC_PROBE_VIABILITY=1 to run \
             (it seeds 10k-250k-row redb stores; keeps `cargo bench`/`cargo test` fast)."
        );
    }
    // Trivial marker so the bench id participates in criterion selection.
    c.bench_function("viability_marker", |b| b.iter(|| black_box(1u32 + 1)));
}

criterion_group!(part_a, codec_size, codec_speed);
criterion_group!(part_b, viability);
criterion_main!(part_a, part_b);
