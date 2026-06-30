//! Native sync protocol for distributed PulseDB instances.
//!
//! This module enables synchronizing data between PulseDB instances
//! across a network — PulseDB's evolution from embedded-only to
//! distributed agentic database.
//!
//! # Architecture
//!
//! ```text
//! Desktop (Tauri)                    Server (Axum)
//! ┌──────────────────┐              ┌──────────────────┐
//! │  PulseDB (local) │              │  PulseDB (server)│
//! │  ┌─────────────┐ │   push/pull  │  ┌─────────────┐ │
//! │  │ SyncManager │◄├─────────────►├──│ SyncManager │ │
//! │  │ (background)│ │  HTTP / WS   │  │ (background)│ │
//! │  └─────────────┘ │              │  └─────────────┘ │
//! └──────────────────┘              └──────────────────┘
//! ```
//!
//! # Feature Flags
//!
//! | Feature | Description |
//! |---------|-------------|
//! | `sync` | Core types, transport trait, sync engine, in-memory transport |
//! | `sync-http` | HTTP transport (reqwest) + server helper for Axum consumers |
//! | `sync-websocket` | WebSocket transport (tokio-tungstenite, future) |
//!
//! # Module Overview
//!
//! **Core** (always with `sync` feature):
//! - `types` — Wire types: `SyncChange`, `SyncPayload`, `InstanceId`, `SyncCursor`
//! - `config` — `SyncConfig`, `SyncDirection`, `ConflictResolution`, `RetryConfig`
//! - `error` — `SyncError` enum (Transport, Timeout, ProtocolVersion, etc.)
//! - `transport` — `SyncTransport` pluggable trait
//! - `transport_mem` — `InMemorySyncTransport` for testing
//! - `guard` — `SyncApplyGuard` thread-local echo prevention
//!
//! **Engine**:
//! - `manager` — `SyncManager`: start/stop/sync_once/initial_sync lifecycle
//! - `applier` — `RemoteChangeApplier`: applies remote changes with idempotency
//! - `progress` — `SyncProgressCallback` for initial sync UI feedback
//!
//! **HTTP** (with `sync-http` feature):
//! - `server` — `SyncServer`: framework-agnostic server handler
//! - `transport_http` — `HttpSyncTransport`: reqwest-based client
//!
//! # WAL Compaction
//!
//! The WAL grows unboundedly as entities are created/updated/deleted.
//! Call [`PulseDB::compact_wal()`](crate::PulseDB::compact_wal) periodically
//! to trim events that all peers have already synced. Compaction uses the
//! min-cursor strategy: only events below the oldest peer's cursor are removed.

pub mod applier;
pub mod config;
pub mod error;
pub mod guard;
pub mod manager;
pub mod progress;
pub(crate) mod pusher;
#[cfg(feature = "sync-http")]
#[cfg_attr(docsrs, doc(cfg(feature = "sync-http")))]
pub mod server;
pub mod transport;
#[cfg(feature = "sync-http")]
#[cfg_attr(docsrs, doc(cfg(feature = "sync-http")))]
pub mod transport_http;
pub mod transport_mem;
pub mod types;

/// Sync protocol version.
///
/// Exchanged during handshake to ensure compatibility between peers.
/// Increment when making breaking changes to the wire format.
///
/// Bumped 2 → 3 in VS-4.0.3: the bincode→postcard serializer swap is *also* a
/// wire-format change, so the protocol version moves in lockstep with
/// [`WIRE_FORMAT_VERSION`].
pub const SYNC_PROTOCOL_VERSION: u32 = 3;

/// Capability advertised by peers that sync reinforcement G-counter fields.
pub const SYNC_CAPABILITY_GCOUNTER_APPLICATIONS: &str = "gcounter-applications";

// ============================================================================
// Wire-format preamble (serializer-independent fail-loud — VS-4.0.3 / C5)
// ============================================================================
//
// The handshake body is framed with a fixed-layout 3-byte preamble that is
// parsed by *raw byte-slicing* BEFORE any deserialize, so two peers running
// different serializers (bincode-era v2 vs postcard-era v3) fail loud with a
// typed `SyncError::WireFormatMismatch` instead of feeding garbage to the
// decoder. On the wire:
//
//     [ SYNC_WIRE_MAGIC[0], SYNC_WIRE_MAGIC[1], wire_format_version ] ++ <body>
//
// Only the handshake (request AND response) carries the preamble. Post-handshake
// push/pull bodies are reached only after a successful handshake pinned the
// version, so they are plain serialized bodies with NO preamble.

/// Fixed magic bytes leading every sync **handshake** wire frame.
///
/// Two distinctive non-ASCII bytes (`0xFE 0xED`, "feed") chosen to be unlikely
/// to collide with the first bytes of a serialized handshake body: postcard
/// frames a `HandshakeRequest` starting with the 16-byte `InstanceId`, whose
/// leading byte is effectively random but very rarely `0xFE`, and a `0xFE 0xED`
/// pair is rarer still — so the magic cheaply catches "this isn't a PulseDB
/// sync preamble at all" (e.g. a pre-4.0 no-preamble peer's raw body) before
/// any version check.
pub const SYNC_WIRE_MAGIC: [u8; 2] = [0xFE, 0xED];

/// Current wire-format version carried in the handshake preamble.
///
/// Moves in lockstep with [`SYNC_PROTOCOL_VERSION`]; a mismatch here is caught
/// pre-deserialize and surfaced as [`error::SyncError::WireFormatMismatch`].
pub const WIRE_FORMAT_VERSION: u8 = 3;

/// Length in bytes of the handshake wire preamble (`magic[2] ++ version[1]`).
pub const SYNC_WIRE_PREAMBLE_LEN: usize = SYNC_WIRE_MAGIC.len() + 1;

/// Prepends the wire preamble (`[magic, magic, version]`) to a serialized
/// handshake `body`, returning the framed bytes ready for the wire.
///
/// Used on BOTH handshake directions (client request encode, server response
/// encode). Push/pull bodies do NOT call this.
pub fn write_wire_preamble(body: &[u8]) -> Vec<u8> {
    let mut framed = Vec::with_capacity(SYNC_WIRE_PREAMBLE_LEN + body.len());
    framed.extend_from_slice(&SYNC_WIRE_MAGIC);
    framed.push(WIRE_FORMAT_VERSION);
    framed.extend_from_slice(body);
    framed
}

/// Parses + validates the 3-byte wire preamble by **raw byte-slicing of
/// `framed[..3]`** and returns the post-preamble body slice on success.
///
/// This MUST run BEFORE any `deserialize(...)` on the handshake body — that
/// ordering is the whole point of the fail-loud design (C5): a serializer
/// mismatch is caught here as a typed [`error::SyncError::WireFormatMismatch`],
/// never as a generic decode error.
///
/// # Errors
/// - [`error::SyncError::WireFormatMismatch`] with `got: None` when the body is
///   shorter than the preamble or the magic bytes don't match (bad/absent magic).
/// - [`error::SyncError::WireFormatMismatch`] with `got: Some(v)` when the magic
///   matches but the `wire_format_version` byte is not [`WIRE_FORMAT_VERSION`].
pub fn read_wire_preamble(framed: &[u8]) -> Result<&[u8], error::SyncError> {
    // Raw byte-slice FIRST — never deserialize before this check.
    if framed.len() < SYNC_WIRE_PREAMBLE_LEN {
        return Err(error::SyncError::wire_format_bad_magic(WIRE_FORMAT_VERSION));
    }
    if framed[..SYNC_WIRE_MAGIC.len()] != SYNC_WIRE_MAGIC {
        return Err(error::SyncError::wire_format_bad_magic(WIRE_FORMAT_VERSION));
    }
    let got = framed[SYNC_WIRE_MAGIC.len()];
    if got != WIRE_FORMAT_VERSION {
        return Err(error::SyncError::wire_format_version(WIRE_FORMAT_VERSION, got));
    }
    Ok(&framed[SYNC_WIRE_PREAMBLE_LEN..])
}

// Re-exports for ergonomic access
pub use config::SyncConfig;
pub use error::SyncError;
pub use guard::{is_sync_applying, SyncApplyGuard};
pub use manager::SyncManager;
pub use progress::SyncProgressCallback;
#[cfg(feature = "sync-http")]
pub use server::SyncServer;
pub use transport::SyncTransport;
#[cfg(feature = "sync-http")]
pub use transport_http::HttpSyncTransport;
pub use transport_mem::InMemorySyncTransport;
pub use types::{
    HandshakeRequest, HandshakeResponse, InstanceId, PullRequest, PullResponse, PushResponse,
    SerializableExperienceUpdate, SyncChange, SyncCursor, SyncEntityType, SyncPayload, SyncStatus,
};
