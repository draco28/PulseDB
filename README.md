# PulseDB

[![CI](https://github.com/pulsehive/pulsedb/actions/workflows/ci.yml/badge.svg)](https://github.com/pulsehive/pulsedb/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/pulsehive-db)](https://crates.io/crates/pulsehive-db)
[![docs.rs](https://docs.rs/pulsehive-db/badge.svg)](https://docs.rs/pulsehive-db)
[![License: AGPL-3.0](https://img.shields.io/badge/License-AGPL--3.0-blue.svg)](LICENSE)
[![MSRV](https://img.shields.io/badge/MSRV-1.89-blue)](Cargo.toml)

**Collective memory for AI agents.** Not message passing. Not RAG. A purpose-built embedded database for multi-agent coordination.

PulseDB is an embedded database purpose-built for agentic AI systems. It provides persistent collective memory for multi-agent systems where agents share experiences and learn from each other — without coordination overhead.

## Features

- **Experience-native storage** — First-class support for agent experiences with importance, confidence, domain tags, and typed variants (insights, errors, patterns, decisions)
- **Integrated vector search** — Built-in HNSW approximate nearest neighbor search for semantic similarity (384-dimensional embeddings)
- **Knowledge graph** — Typed relations between experiences (Supports, Contradicts, Elaborates, Supersedes, Implies, RelatedTo)
- **Real-time notifications** — In-process watch streams via crossbeam channels (<100ns overhead per event) and cross-process change detection
- **Context assembly** — Single API call retrieves similar experiences, recent activity, insights, relations, and active agents
- **SubstrateProvider** — Async trait adapter for agent framework integration
- **Optional ONNX embeddings** — Built-in all-MiniLM-L6-v2 (384d) with automatic model download (`builtin-embeddings` feature)
- **ACID transactions** — redb-backed storage with crash safety via shadow paging

## Quick Start

```rust
use pulsedb::{PulseDB, Config, NewExperience};

// Open or create a database
let db = PulseDB::open("my-agents.db", Config::default())?;

// Create a collective (isolated namespace)
let collective = db.create_collective("my-project")?;

// Record an experience
db.record_experience(NewExperience {
    collective_id: collective,
    content: "Always validate user input before processing".to_string(),
    importance: 0.8,
    embedding: Some(vec![0.1f32; 384]),
    ..Default::default()
})?;

// Search for relevant experiences
let query_embedding = vec![0.1f32; 384];
let results = db.search_similar(collective, &query_embedding, 10)?;

// Clean up
db.close()?;
```

## Installation

Add PulseDB to your `Cargo.toml`:

```toml
[dependencies]
pulsehive-db = "0.1"
```

With built-in embedding generation (no external embedding service needed):

```toml
[dependencies]
pulsehive-db = { version = "0.1", features = ["builtin-embeddings"] }
```

## Performance

Measured on Apple Silicon (M-series), single-threaded:

| Operation | 1K experiences | Target (100K) |
|-----------|---------------|---------------|
| `record_experience` | 5.5 ms | < 10 ms |
| `search_similar` (k=20) | 95 us | < 50 ms |
| `get_context_candidates` | 189 us | < 100 ms |
| `get_experience` by ID | 1.3 us | — |

Run benchmarks yourself: `cargo bench`

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                        CONSUMER APPLICATIONS                     │
│  (Agent Frameworks, Custom Agent Systems, RAG Pipelines)               │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  ┌─────────────────────────────────────────────────────────────┐│
│  │                    PulseDB Public API                        ││
│  │  record_experience()  get_context_candidates()  watch()     ││
│  │  create_collective()  store_relation()  store_insight()     ││
│  └─────────────────────────────────────────────────────────────┘│
│                              │                                   │
│  ┌───────────────────────────┼───────────────────────────────┐  │
│  │                     PULSEDB CORE                           │  │
│  │                           │                                │  │
│  │  ┌─────────────┐  ┌──────┴────────┐  ┌─────────────────┐  │  │
│  │  │  Embedding  │  │  Query Engine │  │  Watch System   │  │  │
│  │  │  Provider   │  │  (candidates) │  │  (crossbeam)    │  │  │
│  │  └─────────────┘  └───────────────┘  └─────────────────┘  │  │
│  │         │                 │                   │            │  │
│  │  ┌──────┴─────────────────┴───────────────────┴──────────┐│  │
│  │  │                   Storage Layer                        ││  │
│  │  │  ┌─────────────┐              ┌─────────────────────┐ ││  │
│  │  │  │    redb     │              │    HNSW Index       │ ││  │
│  │  │  │  (KV store) │              │   (hnsw_rs)         │ ││  │
│  │  │  └─────────────┘              └─────────────────────┘ ││  │
│  │  └────────────────────────────────────────────────────────┘│  │
│  └────────────────────────────────────────────────────────────┘  │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

## Comparison

| Feature | PulseDB | pgvector | sqlite-vss | Qdrant | ChromaDB | LanceDB |
|---------|---------|----------|------------|--------|----------|---------|
| Embedded (no server) | Yes | No | Yes | No | No | Yes |
| Experience-native model | Yes | No | No | No | No | No |
| Vector search | Yes | Yes | Yes | Yes | Yes | Yes |
| Knowledge graph | Yes | No | No | No | No | No |
| Real-time watch | Yes | No | No | No | No | No |
| Context assembly | Yes | No | No | No | No | No |
| ACID transactions | Yes | Yes | Yes | No | No | No |
| Language | Rust | SQL | C/SQL | Rust | Python | Rust |

**PulseDB's unique position**: The only embedded database with experience-native storage, integrated vector + graph search, and real-time awareness primitives — purpose-built for agentic AI.

## Key Concepts

### Collective

A **collective** is an isolated namespace for experiences, typically one per project. Each collective has its own vector index.

### Experience

An **experience** is a unit of learned knowledge: content, embedding, importance, confidence, domain tags, and a typed variant (insight, error pattern, success pattern, etc.).

### SubstrateProvider

The **SubstrateProvider** trait provides an async interface for integrating PulseDB with agent frameworks and orchestration layers. `PulseDBSubstrate` wraps sync operations with `tokio::spawn_blocking` for async compatibility.

## Documentation

- [API Reference (docs.rs)](https://docs.rs/pulsehive-db)
- [CHANGELOG](CHANGELOG.md)

## License

PulseDB is dual-licensed:

- **Open Source**: [GNU Affero General Public License v3.0 (AGPL-3.0)](LICENSE) — free for open-source projects and internal use. If you modify PulseDB and offer it as a network service, you must release your source code under AGPL-3.0.
- **Commercial License**: For proprietary use without AGPL obligations, contact us for a commercial license.

This ensures PulseDB remains free for the community while protecting against unauthorized commercial hosting.
