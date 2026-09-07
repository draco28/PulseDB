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
//! - `types` — Wire types: `SyncChange`, `SyncPayload`, `InstanceId`, `SyncPosition` (+ the persisted `SyncCursor` record)
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
//! min-push-position strategy: only events every peer has acknowledged receiving
//! are removed (pull positions never feed compaction — issue #9).
//!
//! # Wire hygiene
//!
//! The sync server is the network edge of PulseDB's trust boundary (ADR-009),
//! and PulseDB builds no router of its own — so the edge checks below are the
//! ones it owns, and a consumer's framework limits stack on top of them.
//!
//! **Request byte cap (#26).** Every byte-level server handler
//! (`SyncServer::handle_{handshake,push,pull}_bytes`, `sync-http`) compares
//! `bytes.len()` against [`SyncConfig::max_request_bytes`] (default 16 MiB)
//! **before** the wire preamble is read and before any postcard decode. An
//! oversized body is refused with the typed
//! [`SyncError::PayloadTooLarge`]`{ size, max }` — never a decode error, never
//! a partial decode; [`SyncError::is_payload_too_large`] is the hook for a
//! `413 Payload Too Large` mapping. The HTTP transport client applies the same
//! cap to response bodies (a `Content-Length` above the cap is refused unread,
//! a chunked body is read bounded). There is no streaming or chunked decode.
//!
//! **Protocol version and capabilities (#12).** The handshake carries a
//! `protocol_version` that is *checked*: a mismatch reaches the client as the
//! typed [`SyncError::ProtocolVersion`]`{ local, remote }`, never as a reason
//! string inside `SyncError::Handshake`. The handshake's capability list, by
//! contrast, is **informational only** — see
//! [`SYNC_CAPABILITY_GCOUNTER_APPLICATIONS`]. Peers advertise capabilities;
//! nothing is negotiated from them and nothing is refused because of them.
//!
//! **Reinforcement clock skew (#13).** An incoming `last_reinforced` beyond
//! `now + `[`SyncConfig::max_clock_skew_ms`] (default 5 minutes) is logged at
//! `warn` with the peer, the experience id and the skew, and counted in the
//! local-only [`SyncStats::skewed_timestamps`] (`SyncManager::stats()`,
//! `SyncServer::stats()`). It is **never** clamped, rejected or re-timestamped:
//! FR-031's max-merge stores the value byte-for-byte, so convergence is
//! untouched. The bound is advisory until protocol v5 carries a record-level
//! time reference (Release 2).

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
/// Bumped 2 → 3 in VS-4.0.3 (bincode→postcard serializer swap + wire preamble).
/// Bumped 3 → 4 in VS-4.3.2 (key-value `tags` field on `Experience` +
/// `SerializableExperienceUpdate`).
pub const SYNC_PROTOCOL_VERSION: u32 = 4;

/// Capability advertised by peers that sync reinforcement G-counter fields.
///
/// The handshake capability list is **informational and not negotiated**: a
/// peer advertises what it speaks so that operators and logs can see it, but
/// no capability is required, matched, or used to refuse a handshake, and no
/// behaviour is switched on its presence. Compatibility is decided solely by
/// [`SYNC_PROTOCOL_VERSION`] (checked, typed) and the wire preamble
/// ([`WIRE_FORMAT_VERSION`], checked, typed). A capability-driven
/// negotiation would be a protocol change.
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
        return Err(error::SyncError::wire_format_version(
            WIRE_FORMAT_VERSION,
            got,
        ));
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
    SerializableExperienceUpdate, SyncChange, SyncCursor, SyncEntityType, SyncPayload,
    SyncPosition, SyncStats, SyncStatus,
};
