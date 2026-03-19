# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.1] - 2026-03-19

### Fixed
- Race condition in builtin embedding model auto-download when multiple PulseDB instances open concurrently (file lock with double-check pattern)

## [0.2.0] - 2026-03-18

### Added
- `SubstrateProvider::create_collective()` for creating collectives through the async trait
- `SubstrateProvider::get_or_create_collective()` for idempotent collective creation (recommended for SDK consumers)
- `SubstrateProvider::list_collectives()` for listing all collectives
- Auto-download of builtin embedding model when missing (no manual download step needed)

### Breaking
- `SubstrateProvider` trait has 3 new required methods — implementors must add them

## [0.1.1] - 2026-03-15

### Changed
- Improved public documentation for docs.rs readability
- Added docs.rs build configuration for feature-gated items
- Added Feature Flags documentation table to crate-level docs

## [0.1.0] - 2026-03-15

### Added

#### Core
- Database open/close lifecycle with ACID guarantees via redb
- redb storage layer with schema versioning and corruption detection
- Collective CRUD operations for project-level isolation
- Experience CRUD (record, get, update, archive, delete, reinforce)
- Comprehensive input validation for all public APIs
- Built-in ONNX embedding service (all-MiniLM-L6-v2, 384d) with atomic model download (`builtin-embeddings` feature)

#### Search & Retrieval
- HNSW vector index integration for approximate nearest neighbor search (hnsw_rs)
- Similarity search API with cosine distance scoring and domain/type/importance filtering
- Recent experiences API with timestamp-ordered retrieval
- Unified context candidates API aggregating similar, recent, insights, relations, and active agents

#### Knowledge Graph
- Typed experience relations (Supports, Contradicts, Elaborates, Supersedes, Implies, RelatedTo)
- Direction-based relation querying (Outgoing, Incoming, Both)
- Derived insight storage with vector search
- Agent activity tracking with heartbeat and stale detection

#### Real-time & Integration
- In-process watch system for real-time experience notifications via crossbeam channels
- Cross-process change detection via WAL sequence tracking and file lock coordination
- Configurable watch behavior (WatchConfig: in_process toggle, poll interval, buffer size)
- SubstrateProvider async trait and PulseDBSubstrate adapter for agent framework integration

#### Quality
- Error handling audit: comprehensive PulseDBError hierarchy with actionable messages
- All public APIs documented with examples (50 doc tests passing)
- Property-based tests with proptest (7 invariant tests)
- Fuzz testing infrastructure with 3 cargo-fuzz targets
- Test coverage at 89.56% (2033/2270 lines)
- Criterion benchmarks for core operations, mixed workloads, and scaling (1K-100K)
- CI pipeline: 6 jobs (lint, test, MSRV, coverage, security audit, benchmarks)
- CI regression detection with critcmp (10% threshold)

### Performance Targets

| Operation | Target | Measured (1K) |
|-----------|--------|---------------|
| `record_experience` | < 10 ms | 5.5 ms |
| `search_similar` (k=20) | < 50 ms | 95 us |
| `get_context_candidates` | < 100 ms | 189 us |
| `open()` | < 100 ms | < 5 ms |
