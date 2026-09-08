//! Integration tests for Phase 4: HTTP Sync Transport.
//!
//! Spins up a real Axum server with SyncServer handlers, then tests
//! HttpSyncTransport against it.

#![cfg(feature = "sync-http")]

use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Router;
use tokio::net::TcpListener;

use pulsedb::sync::config::{SyncConfig, SyncDirection};
use pulsedb::sync::error::SyncError;
use pulsedb::sync::guard::SyncApplyGuard;
use pulsedb::sync::manager::SyncManager;
use pulsedb::sync::server::SyncServer;
use pulsedb::sync::transport::SyncTransport;
use pulsedb::sync::transport_http::HttpSyncTransport;
use pulsedb::sync::types::{HandshakeRequest, InstanceId, PullRequest, PullResponse, SyncPosition};
use pulsedb::sync::{
    read_wire_preamble, write_wire_preamble, SYNC_PROTOCOL_VERSION, SYNC_WIRE_MAGIC,
    SYNC_WIRE_PREAMBLE_LEN, WIRE_FORMAT_VERSION,
};
use pulsedb::{CollectiveId, Config, NewExperience, PulseDB};
use tempfile::tempdir;

// ============================================================================
// Axum handlers (test server)
// ============================================================================

/// Maps a typed `SyncError` from the byte-level handlers onto an HTTP status:
/// an oversized body is `413 Payload Too Large`, everything else is `400`.
fn status_for(err: SyncError) -> StatusCode {
    if err.is_payload_too_large() {
        StatusCode::PAYLOAD_TOO_LARGE
    } else {
        StatusCode::BAD_REQUEST
    }
}

async fn handle_health(State(server): State<Arc<SyncServer>>) -> StatusCode {
    match server.handle_health() {
        Ok(()) => StatusCode::OK,
        Err(_) => StatusCode::SERVICE_UNAVAILABLE,
    }
}

async fn handle_handshake(
    State(server): State<Arc<SyncServer>>,
    body: Bytes,
) -> Result<Vec<u8>, StatusCode> {
    server.handle_handshake_bytes(&body).map_err(status_for)
}

async fn handle_push(
    State(server): State<Arc<SyncServer>>,
    body: Bytes,
) -> Result<Vec<u8>, StatusCode> {
    server.handle_push_bytes(&body).map_err(status_for)
}

async fn handle_pull(
    State(server): State<Arc<SyncServer>>,
    body: Bytes,
) -> Result<Vec<u8>, StatusCode> {
    server.handle_pull_bytes(&body).map_err(status_for)
}

fn sync_router(server: Arc<SyncServer>) -> Router {
    Router::new()
        .route("/sync/health", get(handle_health))
        .route("/sync/handshake", post(handle_handshake))
        .route("/sync/push", post(handle_push))
        .route("/sync/pull", post(handle_pull))
        .with_state(server)
}

// ============================================================================
// Test helpers
// ============================================================================

struct TestServer {
    base_url: String,
    db: Arc<PulseDB>,
    server: Arc<SyncServer>,
    _dir: tempfile::TempDir,
}

async fn start_test_server() -> TestServer {
    let dir = tempdir().unwrap();
    let db = Arc::new(PulseDB::open(dir.path().join("server.db"), Config::default()).unwrap());
    let server = Arc::new(SyncServer::new(Arc::clone(&db), SyncConfig::default()));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{}", addr);

    let router = sync_router(Arc::clone(&server));
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    // Give server a moment to start
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

    TestServer {
        base_url,
        db,
        server,
        _dir: dir,
    }
}

fn minimal_exp(cid: CollectiveId) -> NewExperience {
    NewExperience {
        collective_id: cid,
        content: format!("http-test-{}", uuid::Uuid::now_v7()),
        embedding: Some(vec![0.1f32; 384]),
        ..Default::default()
    }
}

/// Builds a bare in-process `SyncServer` (no HTTP) for byte-level handler tests
/// that need the typed `SyncError` back rather than an HTTP status code.
fn in_process_server() -> (Arc<SyncServer>, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let db = Arc::new(PulseDB::open(dir.path().join("server.db"), Config::default()).unwrap());
    let server = Arc::new(SyncServer::new(db, SyncConfig::default()));
    (server, dir)
}

/// A valid postcard-encoded handshake request body (no preamble), used as the
/// payload under test for preamble-framing cases.
fn handshake_body_bytes() -> Vec<u8> {
    let request = HandshakeRequest {
        instance_id: InstanceId::new(),
        protocol_version: SYNC_PROTOCOL_VERSION,
        capabilities: vec!["push".into()],
    };
    postcard::to_allocvec(&request).unwrap()
}

// ============================================================================
// Tests
// ============================================================================

#[tokio::test]
async fn test_http_health_check() {
    let server = start_test_server().await;
    let transport = HttpSyncTransport::new(&server.base_url);

    let result = transport.health_check().await;
    assert!(result.is_ok(), "Health check should succeed");
}

#[tokio::test]
async fn test_http_handshake() {
    let server = start_test_server().await;
    let transport = HttpSyncTransport::new(&server.base_url);

    let request = HandshakeRequest {
        instance_id: InstanceId::new(),
        protocol_version: SYNC_PROTOCOL_VERSION,
        capabilities: vec!["push".into(), "pull".into()],
    };

    let response = transport.handshake(request).await.unwrap();
    assert!(response.accepted);
    assert_eq!(response.protocol_version, SYNC_PROTOCOL_VERSION);
    assert_ne!(response.instance_id, InstanceId::nil());
}

#[tokio::test]
async fn test_http_push_and_pull_roundtrip() {
    let server = start_test_server().await;
    let transport = HttpSyncTransport::new(&server.base_url);

    // Create data on server
    let cid = server.db.create_collective("http-test").unwrap();
    let _exp_id = server.db.record_experience(minimal_exp(cid)).unwrap();

    // Pull changes via HTTP
    let pull_request = PullRequest {
        cursor: SyncPosition::new(InstanceId::new(), 0),
        batch_size: 100,
        collectives: None,
    };
    let pull_response = transport.pull_changes(pull_request).await.unwrap();

    // Should have collective + experience
    assert!(
        !pull_response.changes.is_empty(),
        "Should have changes to pull"
    );
    assert!(pull_response.changes.len() >= 2); // collective + experience at minimum
}

/// **This test PINS A DEFECT, not desired behaviour — issue #96.** A catch-up
/// over HTTP pulls the server's data and reports honestly that it did NOT
/// complete, because one change cannot apply on this transport.
///
/// `Experience::embedding` is `#[serde(skip)]` (embeddings live in their own
/// table and the storage layer rejoins them on read), so an `ExperienceCreated`
/// crossing any SERIALIZING transport arrives with an empty vector and the
/// applier's create fails the collective's dimension check. In-memory sync is
/// unaffected — it hands the struct over without a serialize step, which is why
/// the engine tests do not see this.
///
/// The remaining defect is the WIRE FORMAT: `SyncPayload` does not carry
/// embeddings, so the experience cannot cross this transport at all. What is no
/// longer part of it (PR #88 class L, then class P) is the lying: `initial_sync`
/// does not report `Ok(())` over the top of the failure, and the failed create
/// no longer leaves the experience RECORD behind without its vector. The
/// collective arrives; the experience does not arrive at all.
///
/// This test is #96's tripwire. When the sync payload carries embeddings, it
/// goes RED here: FLIP the expectations back — `initial_sync` to `Ok(())`, the
/// experience to present and the search to a hit — rather than deleting the
/// test, which is what proves the fix landed.
#[tokio::test]
async fn test_http_full_sync_via_manager_pins_the_missing_embedding_defect() {
    let server = start_test_server().await;
    let dir_client = tempdir().unwrap();
    let db_client =
        Arc::new(PulseDB::open(dir_client.path().join("client.db"), Config::default()).unwrap());

    let transport = HttpSyncTransport::new(&server.base_url);
    let config = SyncConfig::default();
    let mut manager = SyncManager::new(Arc::clone(&db_client), Box::new(transport), config);

    // Create data on server
    let cid = server.db.create_collective("full-sync").unwrap();
    let exp_id = server.db.record_experience(minimal_exp(cid)).unwrap();

    // Client does initial sync to pull all server data
    let error = manager
        .initial_sync(None)
        .await
        .expect_err("the experience cannot apply without its embedding");
    assert!(
        error.is_catch_up_incomplete(),
        "an unappliable change must surface, not be reported as a completed \
         catch-up, got: {error}"
    );
    assert!(
        error.to_string().contains("failed to apply"),
        "the error must name what stopped it, got: {error}"
    );

    // What DID arrive: the collective, and nothing else. The experience change
    // could not be applied, so none of it was written — no record, and so
    // nothing for a semantic search over the synced collective to find.
    assert!(
        db_client.get_collective(cid).unwrap().is_some(),
        "Collective should sync via HTTP"
    );
    assert!(
        db_client.get_experience(exp_id).unwrap().is_none(),
        "a create that could not be indexed must leave no record behind — the \
         experience is absent, not present-but-unsearchable"
    );
    assert_eq!(
        db_client
            .search_similar(cid, &vec![0.1f32; 384], 5)
            .unwrap()
            .len(),
        0,
        "the experience never arrived, so there is nothing to find — the sync \
         payload does not carry embeddings"
    );
}

/// The consequence that makes the class worth having: the failure is
/// REPEATABLE, not one-shot.
///
/// The applier's `ExperienceCreated` arm short-circuits on
/// `get_experience(id).is_some()`. While a failed create left its record in the
/// store, the retry therefore resolved as applied-or-skipped, the cursors moved
/// past the change, and the second catch-up reported `Ok(())` over an
/// experience that had never been usable. With nothing written, the create path
/// is re-entered on every attempt and the same honest error comes back.
#[tokio::test]
async fn http_catch_up_reports_the_same_failure_on_every_attempt() {
    let server = start_test_server().await;
    let dir_client = tempdir().unwrap();
    let db_client =
        Arc::new(PulseDB::open(dir_client.path().join("client.db"), Config::default()).unwrap());

    let transport = HttpSyncTransport::new(&server.base_url);
    let mut manager = SyncManager::new(
        Arc::clone(&db_client),
        Box::new(transport),
        SyncConfig::default(),
    );

    let cid = server.db.create_collective("repeatable-catch-up").unwrap();
    let exp_id = server.db.record_experience(minimal_exp(cid)).unwrap();

    let first = manager
        .initial_sync(None)
        .await
        .expect_err("the experience cannot apply without its embedding");
    assert!(
        first.is_catch_up_incomplete(),
        "first catch-up must report the incomplete run, got: {first}"
    );

    let second = manager
        .initial_sync(None)
        .await
        .expect_err("the same change is still unappliable on the second attempt");
    assert!(
        second.is_catch_up_incomplete(),
        "a repeated catch-up must report the SAME incomplete run, not `Ok(())` \
         off the back of a half-written record, got: {second}"
    );

    assert!(
        db_client.get_experience(exp_id).unwrap().is_none(),
        "no attempt may leave a record behind"
    );
}

/// **Acceptance test for issue #96 — FAILING BY DESIGN, so `#[ignore]`d.**
///
/// This is what an HTTP catch-up is supposed to do, and what the test above
/// asserted before it was rewritten to pin the defect: pull the collective and
/// the experience, report `Ok(())`, and leave the experience findable by
/// semantic search on the client.
///
/// It fails today because `Experience::embedding` is `#[serde(skip)]`, so the
/// experience crosses the wire without its vector, the applier's create fails
/// the dimension check, and the record lands unsearchable.
///
/// Whoever fixes #96 REMOVES the `#[ignore]` here and flips the expectations of
/// the characterization test above back to success. Neither test is to be
/// deleted: this one proves the fix works, that one proves the defect is gone.
#[tokio::test]
#[ignore = "issue #96: experiences lose their embeddings over any serializing \
            transport; un-ignore when the sync payload carries them"]
async fn acceptance_96_http_catch_up_delivers_a_searchable_experience() {
    let server = start_test_server().await;
    let dir_client = tempdir().unwrap();
    let db_client =
        Arc::new(PulseDB::open(dir_client.path().join("client.db"), Config::default()).unwrap());

    let transport = HttpSyncTransport::new(&server.base_url);
    let config = SyncConfig::default();
    let mut manager = SyncManager::new(Arc::clone(&db_client), Box::new(transport), config);

    // Create data on server
    let cid = server.db.create_collective("full-sync").unwrap();
    let exp_id = server.db.record_experience(minimal_exp(cid)).unwrap();

    // Client does initial sync to pull all server data
    manager
        .initial_sync(None)
        .await
        .expect("a catch-up that pulled everything must report completion");

    // Client should have the collective and experience
    assert!(
        db_client.get_collective(cid).unwrap().is_some(),
        "Collective should sync via HTTP"
    );
    assert!(
        db_client.get_experience(exp_id).unwrap().is_some(),
        "Experience should sync via HTTP"
    );
    // ...and the experience must arrive WITH its embedding, so it is findable.
    let hits = db_client
        .search_similar(cid, &vec![0.1f32; 384], 5)
        .unwrap();
    assert!(
        hits.iter().any(|hit| hit.experience.id == exp_id),
        "the synced experience must be searchable on the client, which needs \
         its embedding to have crossed the wire"
    );
}

#[tokio::test]
async fn test_http_reinforcement_gcounter_converges_exact_total() {
    let server = start_test_server().await;
    let dir_client = tempdir().unwrap();
    let db_client =
        Arc::new(PulseDB::open(dir_client.path().join("client.db"), Config::default()).unwrap());

    let transport = HttpSyncTransport::new(&server.base_url);
    let mut manager = SyncManager::new(
        Arc::clone(&db_client),
        Box::new(transport),
        SyncConfig::default(),
    );

    let cid = server.db.create_collective("http-gcounter").unwrap();
    let exp_id = server.db.record_experience(minimal_exp(cid)).unwrap();

    let seed = server.db.get_experience(exp_id).unwrap().unwrap();
    let guard = SyncApplyGuard::enter();
    db_client.apply_synced_experience(seed).unwrap();
    drop(guard);

    server.db.reinforce_experience(exp_id).unwrap();
    db_client.reinforce_experience(exp_id).unwrap();
    db_client.reinforce_experience(exp_id).unwrap();

    manager.sync_once().await.unwrap();

    let server_exp = server.db.get_experience(exp_id).unwrap().unwrap();
    let client_exp = db_client.get_experience(exp_id).unwrap().unwrap();
    assert_eq!(server_exp.applications(), 3);
    assert_eq!(client_exp.applications(), 3);
    assert_eq!(server_exp.applications, client_exp.applications);
}

#[tokio::test]
async fn test_http_auth_token() {
    // This test just verifies the transport sends the header without error.
    // Full auth verification would require server-side middleware.
    let server = start_test_server().await;
    let transport = HttpSyncTransport::with_auth(&server.base_url, "test-token-123");

    // Health check should still work (server doesn't enforce auth)
    let result = transport.health_check().await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_http_error_bad_url() {
    let transport = HttpSyncTransport::new("http://127.0.0.1:1"); // port 1 should fail

    let result = transport.health_check().await;
    assert!(result.is_err(), "Bad URL should fail");
}

/// The PUSH direction meets the same wire-format defect as the pull one —
/// issue #96, and the same note applies: FLIP the experience expectation back
/// to `is_some()` when `SyncPayload` carries embeddings.
///
/// `Experience::embedding` is `#[serde(skip)]`, so the experience reaches the
/// server's applier with an empty vector and its create is refused by the
/// collective's dimension check. The collective, which carries no embedding,
/// goes across fine. Nothing partial is left on the server: the create is
/// rejected before anything is written.
#[tokio::test]
async fn test_http_push_to_server() {
    let server = start_test_server().await;
    let dir_client = tempdir().unwrap();
    let db_client =
        Arc::new(PulseDB::open(dir_client.path().join("client.db"), Config::default()).unwrap());

    let transport = HttpSyncTransport::new(&server.base_url);
    let config = SyncConfig {
        direction: SyncDirection::PushOnly,
        ..SyncConfig::default()
    };
    let mut manager = SyncManager::new(Arc::clone(&db_client), Box::new(transport), config);

    // Create data on client
    let cid = db_client.create_collective("push-test").unwrap();
    let exp_id = db_client.record_experience(minimal_exp(cid)).unwrap();

    // Push to server
    manager.sync_once().await.unwrap();

    // Server should have the collective; the experience cannot cross this
    // transport at all (#96) and must not arrive half-formed.
    assert!(
        server.db.get_collective(cid).unwrap().is_some(),
        "Collective should be pushed to server"
    );
    assert!(
        server.db.get_experience(exp_id).unwrap().is_none(),
        "the experience loses its embedding on the wire (#96), so its create is \
         refused — and refusing it must leave NO record on the server"
    );

    // The change that failed is not acknowledged, so it stays pushable: the
    // client's push position stopped below it and `compact_wal` cannot drop it.
    let peer_id = server.db.instance_id();
    let cursor = db_client
        .storage_for_test()
        .load_sync_cursor(&peer_id)
        .unwrap()
        .expect("a push cycle records a cursor for the peer");
    assert!(
        cursor.push_sequence < 2,
        "the acknowledged push position must stay below the experience event, \
         so the change is retried rather than compacted away, got {}",
        cursor.push_sequence
    );
}

// ============================================================================
// Wire-format preamble tests (VS-4.0.3 / C5 — serializer-independent fail-loud)
// ============================================================================

/// (a) Preamble round-trip: a correctly-framed handshake body decodes cleanly,
/// and the server's response is itself preamble-framed (both directions).
#[tokio::test]
async fn test_wire_preamble_roundtrip_both_directions() {
    let (server, _dir) = in_process_server();

    // Client side: frame a valid postcard body with the preamble.
    let framed_request = write_wire_preamble(&handshake_body_bytes());
    assert_eq!(
        &framed_request[..2],
        &SYNC_WIRE_MAGIC,
        "magic leads the frame"
    );
    assert_eq!(
        framed_request[2], WIRE_FORMAT_VERSION,
        "version byte follows magic"
    );

    // Server accepts the framed request and returns a framed response.
    let framed_response = server
        .handle_handshake_bytes(&framed_request)
        .expect("valid framed handshake must succeed");

    // The RESPONSE also carries the preamble (the other direction).
    let payload =
        read_wire_preamble(&framed_response).expect("server response must carry a valid preamble");
    let response: pulsedb::sync::types::HandshakeResponse =
        postcard::from_bytes(payload).expect("response body decodes after preamble strip");
    assert!(response.accepted, "matched-version handshake is accepted");
    assert_eq!(response.protocol_version, SYNC_PROTOCOL_VERSION);
}

/// (b) Bad-magic body → typed `WireFormatMismatch`, NOT `Serialization`.
/// This proves the preamble is parsed (raw byte-slice of `body[..3]`) BEFORE
/// any deserialize: a postcard-garbage leading body never reaches the decoder.
#[tokio::test]
async fn test_wire_preamble_bad_magic_is_typed_not_serialization() {
    let (server, _dir) = in_process_server();

    // A body with WRONG magic but otherwise a plausible postcard payload.
    let mut bad = handshake_body_bytes();
    let mut framed = vec![0x00, 0x01, WIRE_FORMAT_VERSION]; // wrong magic bytes
    framed.append(&mut bad);

    let err = server
        .handle_handshake_bytes(&framed)
        .expect_err("bad magic must fail");

    assert!(
        err.is_wire_format_mismatch(),
        "bad magic must be the typed WireFormatMismatch, got: {err:?}"
    );
    assert!(
        matches!(err, SyncError::WireFormatMismatch { got: None, .. }),
        "bad magic carries got: None (no trustworthy version), got: {err:?}"
    );
    // The whole point: it is NOT a generic Serialization error.
    assert!(
        !matches!(err, SyncError::Serialization(_)),
        "bad magic must NOT collapse to a generic Serialization error"
    );
}

/// (c) Wrong `wire_format_version` (valid magic) → typed `WireFormatMismatch`.
#[tokio::test]
async fn test_wire_preamble_wrong_version_is_typed() {
    let (server, _dir) = in_process_server();

    let mut body = handshake_body_bytes();
    let mut framed = Vec::new();
    framed.extend_from_slice(&SYNC_WIRE_MAGIC); // valid magic
    framed.push(WIRE_FORMAT_VERSION.wrapping_sub(1)); // wrong version (e.g. v2)
    framed.append(&mut body);

    let err = server
        .handle_handshake_bytes(&framed)
        .expect_err("wrong wire version must fail");

    assert!(
        err.is_wire_format_mismatch(),
        "wrong version is typed, got: {err:?}"
    );
    assert!(
        matches!(
            err,
            SyncError::WireFormatMismatch {
                expected,
                got: Some(g)
            } if expected == WIRE_FORMAT_VERSION && g == WIRE_FORMAT_VERSION.wrapping_sub(1)
        ),
        "wrong version reports expected + observed bytes, got: {err:?}"
    );
}

/// (d) Mixed-version fail-loud: a v2/bincode-era-style body (no preamble at all)
/// fed to the v3 server fails loud with a typed error — no panic, no silent
/// accept. A pre-4.0 peer's raw postcard/bincode body has no `0xFE 0xED` magic.
#[tokio::test]
async fn test_mixed_version_no_preamble_body_fails_loud() {
    let (server, _dir) = in_process_server();

    // A pre-preamble peer sends a raw (unframed) serialized handshake body.
    let raw_no_preamble = handshake_body_bytes();

    let err = server
        .handle_handshake_bytes(&raw_no_preamble)
        .expect_err("a no-preamble body must fail loud against the v3 server");

    assert!(
        err.is_wire_format_mismatch(),
        "no-preamble body fails loud as WireFormatMismatch (not silent accept / not panic), got: {err:?}"
    );

    // A too-short body (fewer than the 3 preamble bytes) is also bad-magic.
    let truncated = vec![SYNC_WIRE_MAGIC[0]]; // 1 byte, < SYNC_WIRE_PREAMBLE_LEN
    assert!(truncated.len() < SYNC_WIRE_PREAMBLE_LEN);
    let err2 = server
        .handle_handshake_bytes(&truncated)
        .expect_err("a truncated body must fail loud");
    assert!(
        matches!(err2, SyncError::WireFormatMismatch { got: None, .. }),
        "truncated body is bad-magic typed error, got: {err2:?}"
    );
}

/// (e) Keep the existing in-band `protocol_version`-mismatch path green: a
/// correctly-framed handshake whose body advertises a different protocol
/// version still flows through to the soft `accepted: false` response — the new
/// preamble does NOT mask the protocol-semantics negotiation.
#[tokio::test]
async fn test_protocol_version_mismatch_still_soft_rejects_through_preamble() {
    let (server, _dir) = in_process_server();

    // Valid preamble + valid postcard body, but a mismatched protocol_version.
    let request = HandshakeRequest {
        instance_id: InstanceId::new(),
        protocol_version: SYNC_PROTOCOL_VERSION + 99, // semantic mismatch, valid wire
        capabilities: vec![],
    };
    let body = postcard::to_allocvec(&request).unwrap();
    let framed = write_wire_preamble(&body);

    let framed_response = server
        .handle_handshake_bytes(&framed)
        .expect("a wire-valid handshake reaches the protocol-version gate");

    let payload = read_wire_preamble(&framed_response).expect("response is framed");
    let response: pulsedb::sync::types::HandshakeResponse = postcard::from_bytes(payload).unwrap();
    assert!(
        !response.accepted,
        "protocol-version mismatch still yields the soft accepted:false path"
    );
    assert!(
        response.reason.is_some(),
        "soft rejection carries a reason string"
    );
}

/// Sanity: a real cross-version HTTP exchange fails loud at the client too —
/// a no-preamble (pre-4.0-style) request POSTed to the v3 server yields an
/// error status, and the v3 client rejects a mangled response preamble.
#[tokio::test]
async fn test_http_handshake_happy_path_carries_preamble() {
    let server = start_test_server().await;
    let transport = HttpSyncTransport::new(&server.base_url);

    let request = HandshakeRequest {
        instance_id: InstanceId::new(),
        protocol_version: SYNC_PROTOCOL_VERSION,
        capabilities: vec!["push".into()],
    };

    // The transport frames the request preamble and validates the response
    // preamble end-to-end over real HTTP; a clean round-trip proves both legs.
    let response = transport
        .handshake(request)
        .await
        .expect("framed handshake round-trips over HTTP");
    assert!(response.accepted);
    assert_eq!(response.protocol_version, SYNC_PROTOCOL_VERSION);
}

// ============================================================================
// Wire hygiene (r1.s1.w3 — #26 request byte cap)
// ============================================================================

/// Builds an in-process `SyncServer` with an explicit request byte cap.
///
/// The caps here are bytes, not megabytes — far below any value
/// `SyncConfig::validate` would accept beside a non-zero `batch_size`. That is
/// the point: these tests exercise the byte-cap refusal itself, so the config is
/// constructed directly and deliberately never validated.
fn in_process_server_with_cap(max_request_bytes: usize) -> (Arc<SyncServer>, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let db = Arc::new(PulseDB::open(dir.path().join("server.db"), Config::default()).unwrap());
    let config = SyncConfig {
        max_request_bytes,
        ..SyncConfig::default()
    };
    let server = Arc::new(SyncServer::new(db, config));
    (server, dir)
}

fn assert_payload_too_large(err: &SyncError, expected_size: usize, expected_max: usize, leg: &str) {
    assert!(
        matches!(
            err,
            SyncError::PayloadTooLarge { size, max }
                if *size == expected_size && *max == expected_max
        ),
        "{leg}: expected typed PayloadTooLarge {{ size: {expected_size}, max: {expected_max} }}, got {err:?}"
    );
    assert!(err.is_payload_too_large(), "{leg}: is_payload_too_large()");
    assert!(
        !matches!(
            err,
            SyncError::Serialization(_) | SyncError::WireFormatMismatch { .. }
        ),
        "{leg}: the cap must not collapse into a decode-side error"
    );
}

/// #26: every byte-level server handler compares `bytes.len()` against
/// `SyncConfig::max_request_bytes` BEFORE the body reaches postcard (and, for
/// the handshake, before the preamble read). The refused bodies below are all
/// decodable — postcard ignores trailing bytes and a zero-filled body is a
/// valid empty `Vec<SyncChange>` — so only a pre-decode length check can
/// produce the typed `PayloadTooLarge`.
#[tokio::test]
async fn oversized_request_is_refused_before_decode() {
    const CAP: usize = 64;
    let (server, _dir) = in_process_server_with_cap(CAP);

    // A wire-valid framed handshake under the cap is accepted as before.
    let framed = write_wire_preamble(&handshake_body_bytes());
    assert!(
        framed.len() <= CAP,
        "fixture must fit under the cap: {} > {CAP}",
        framed.len()
    );
    server
        .handle_handshake_bytes(&framed)
        .expect("in-bound framed handshake is accepted");

    // The same body padded one byte past the cap. It still decodes under a
    // larger cap, which is what proves the refusal happens before decode.
    let mut padded = framed.clone();
    padded.resize(CAP + 1, 0);
    let (lenient, _dir_lenient) = in_process_server_with_cap(1024);
    lenient
        .handle_handshake_bytes(&padded)
        .expect("the padded body is decodable — trailing bytes are ignored by postcard");

    let err = server
        .handle_handshake_bytes(&padded)
        .expect_err("oversized handshake must be refused");
    assert_payload_too_large(&err, CAP + 1, CAP, "handshake");

    // Push and pull bodies (no preamble) are capped the same way.
    let oversized = vec![0u8; CAP + 1];
    let err = server
        .handle_push_bytes(&oversized)
        .expect_err("oversized push must be refused");
    assert_payload_too_large(&err, CAP + 1, CAP, "push");
    let err = server
        .handle_pull_bytes(&oversized)
        .expect_err("oversized pull must be refused");
    assert_payload_too_large(&err, CAP + 1, CAP, "pull");

    // The cap is inclusive: a body exactly at the cap reaches the decoder
    // (a zero-filled body is an empty change batch).
    let at_cap = vec![0u8; CAP];
    let encoded = server
        .handle_push_bytes(&at_cap)
        .expect("a body exactly at the cap reaches the decoder");
    let response: pulsedb::sync::types::PushResponse = postcard::from_bytes(&encoded).unwrap();
    assert_eq!(response.accepted, 0);
}

/// #26, client side: the HTTP transport applies the same cap to response
/// bodies — a `Content-Length` above the cap is refused without reading the
/// body, and a chunked (no `Content-Length`) body is read bounded and refused
/// once it crosses the cap.
#[tokio::test]
async fn client_refuses_oversized_response_body() {
    const CAP: usize = 1024;
    const OVERSIZED: usize = 64 * 1024;

    // A hostile "server": /sync/pull answers with an oversized fixed-length
    // body; /sync/push streams an oversized body without a Content-Length.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = Router::new()
        .route("/sync/pull", post(|| async { vec![0u8; OVERSIZED] }))
        .route(
            "/sync/push",
            post(|| async {
                let chunks =
                    (0..(OVERSIZED / 256)).map(|_| Ok::<_, std::io::Error>(vec![0u8; 256]));
                Body::from_stream(futures::stream::iter(chunks))
            }),
        );
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

    let transport = HttpSyncTransport::new(format!("http://{addr}")).with_max_response_bytes(CAP);
    assert_eq!(transport.max_response_bytes(), CAP);

    let pull_request = PullRequest {
        cursor: SyncPosition::new(InstanceId::new(), 0),
        batch_size: 10,
        collectives: None,
    };
    let err = transport
        .pull_changes(pull_request)
        .await
        .expect_err("an oversized Content-Length response must be refused");
    assert!(
        matches!(err, SyncError::PayloadTooLarge { size, max } if size == OVERSIZED && max == CAP),
        "pull (Content-Length): expected PayloadTooLarge {{ size: {OVERSIZED}, max: {CAP} }}, got {err:?}"
    );

    let err = transport
        .push_changes(Vec::new())
        .await
        .expect_err("an oversized chunked response must be refused");
    assert!(
        matches!(err, SyncError::PayloadTooLarge { size, max } if size > CAP && max == CAP),
        "push (chunked): expected PayloadTooLarge {{ size > {CAP}, max: {CAP} }}, got {err:?}"
    );
}

// ============================================================================
// Wire hygiene (r1.s1.w3 — #12 typed client protocol-version mismatch)
// ============================================================================

/// #12: a server advertising a different `SYNC_PROTOCOL_VERSION` reaches the
/// client as the typed `SyncError::ProtocolVersion { local, remote }`, not as
/// a reason string folded into `SyncError::Handshake`. A real `SyncServer`
/// answers a mismatch with the soft `accepted: false` path (see
/// `test_protocol_version_mismatch_still_soft_rejects_through_preamble`); that
/// path must no longer shadow the typed variant on the client.
#[tokio::test]
async fn client_reports_protocol_version_mismatch_typed() {
    const REMOTE_VERSION: u32 = SYNC_PROTOCOL_VERSION + 1;

    // A peer speaking the same wire format but a different protocol version.
    // It answers exactly as `SyncServer::handle_handshake` does on mismatch:
    // `accepted: false`, ITS protocol version, and a human-readable reason.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = Router::new().route(
        "/sync/handshake",
        post(|| async {
            let response = pulsedb::sync::types::HandshakeResponse {
                instance_id: InstanceId::new(),
                protocol_version: REMOTE_VERSION,
                accepted: false,
                reason: Some(format!(
                    "Protocol version mismatch: server v{REMOTE_VERSION}, client v{SYNC_PROTOCOL_VERSION}"
                )),
            };
            write_wire_preamble(&postcard::to_allocvec(&response).unwrap())
        }),
    );
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

    let dir = tempdir().unwrap();
    let db = Arc::new(PulseDB::open(dir.path().join("client.db"), Config::default()).unwrap());
    let transport = HttpSyncTransport::new(format!("http://{addr}"));
    let mut manager = SyncManager::new(Arc::clone(&db), Box::new(transport), SyncConfig::default());

    let err = manager
        .sync_once()
        .await
        .expect_err("a version-mismatched peer must fail the handshake");
    assert!(
        matches!(
            err,
            SyncError::ProtocolVersion { local, remote }
                if local == SYNC_PROTOCOL_VERSION && remote == REMOTE_VERSION
        ),
        "expected typed ProtocolVersion {{ local: {SYNC_PROTOCOL_VERSION}, remote: {REMOTE_VERSION} }}, got {err:?}"
    );
    assert!(
        !matches!(err, SyncError::Handshake(_)),
        "the mismatch must not be a reason string inside Handshake(_), got {err:?}"
    );

    // `start()` goes through the same handshake and reports the same variant.
    let err = manager
        .start()
        .await
        .expect_err("start() fails the handshake the same way");
    assert!(
        matches!(err, SyncError::ProtocolVersion { .. }),
        "start(): expected typed ProtocolVersion, got {err:?}"
    );
}

// ============================================================================
// Skew visibility (r1.s1.w3 — #13, veto fold C2) on the push/server side
// ============================================================================

/// A pushed change whose `last_reinforced` lies beyond
/// `now + max_clock_skew_ms` shows up in `SyncServer::stats()` and is merged
/// unchanged. `skewed_timestamps` is local-only: `PushResponse` on the wire is
/// untouched.
#[tokio::test]
async fn server_stats_count_skewed_last_reinforced() {
    use std::collections::BTreeMap;

    use pulsedb::sync::types::{
        SerializableExperienceUpdate, SyncChange, SyncEntityType, SyncPayload, SyncStats,
    };
    use pulsedb::Timestamp;

    let server = start_test_server().await;
    let transport = HttpSyncTransport::new(&server.base_url);
    let cid = server.db.create_collective("skew-stats-http").unwrap();
    let exp_id = server.db.record_experience(minimal_exp(cid)).unwrap();
    assert_eq!(server.server.stats(), SyncStats::default());

    let allowance = i64::try_from(SyncConfig::default().max_clock_skew_ms).unwrap();
    let skewed = Timestamp::from_millis(Timestamp::now().as_millis() + allowance + 86_400_000);
    let peer = InstanceId::new();
    let change = SyncChange {
        sequence: 7,
        source_instance: peer,
        collective_id: cid,
        entity_type: SyncEntityType::Experience,
        payload: SyncPayload::ExperienceUpdated {
            id: exp_id,
            update: SerializableExperienceUpdate {
                applications: Some(BTreeMap::from([(peer, 3)])),
                last_reinforced: Some(skewed),
                ..Default::default()
            },
            timestamp: Timestamp::now(),
        },
        timestamp: Timestamp::now(),
    };

    let response = transport.push_changes(vec![change]).await.unwrap();
    assert_eq!(response.accepted, 1);
    assert_eq!(
        server.server.stats(),
        SyncStats {
            skewed_timestamps: 1
        },
        "the skewed reinforcement is visible in the server's stats"
    );
    let stored = server.db.get_experience(exp_id).unwrap().unwrap();
    assert_eq!(stored.last_reinforced, skewed, "counted, not clamped");
    assert_eq!(stored.applications.get(&peer), Some(&3));
}

// ============================================================================
// Pull page saturation (PR #88 review, class Q)
// ============================================================================

/// WAL events one server-side pull polls in a single page.
///
/// Mirrors `SyncServer`'s own `PULL_PAGE_EVENT_LIMIT`, which is private to the
/// server: a fixture that reaches this many events is exercising a FULL poll
/// page, the only state from which `has_more` cannot be read off the batch.
const SERVER_PULL_PAGE_EVENTS: usize = 1000;

/// Fills a store's WAL with `count` collective-create events, in WAL order.
///
/// A collective create is the cheapest write that still yields a `SyncChange`
/// (no embedding, no vector on the wire), and it is the payload that survives a
/// serializing transport intact — issue #96 keeps experiences off it — so one
/// fixture serves both the `handle_pull` assertions and the catch-up that runs
/// over real HTTP.
fn fill_wal_with_collectives(db: &PulseDB, count: usize) -> Vec<CollectiveId> {
    (0..count)
        .map(|i| db.create_collective(&format!("page-fill-{i}")).unwrap())
        .collect()
}

/// One pull straight at the server handler, bypassing HTTP.
fn pull_page(
    server: &SyncServer,
    from: u64,
    batch_size: usize,
    collectives: Option<Vec<CollectiveId>>,
) -> PullResponse {
    server
        .handle_pull(PullRequest {
            cursor: SyncPosition::new(server.instance_id(), from),
            batch_size,
            collectives,
        })
        .expect("handle_pull")
}

/// A pull-only client whose `batch_size` sits at or above the server's poll
/// page. `validate()` is asserted here because it is the point of the class:
/// this is a SUPPORTED configuration, not an exotic one — it only needs a
/// `max_request_bytes` that matches the batch it can produce.
fn wide_batch_config(batch_size: usize) -> SyncConfig {
    let config = SyncConfig {
        direction: SyncDirection::PullOnly,
        batch_size,
        max_request_bytes: batch_size
            * pulsedb::sync::config::MAX_EXPERIENCE_WIRE_BYTES_EXCLUDING_APPLICATIONS,
        ..SyncConfig::default()
    };
    config
        .validate()
        .expect("a wide batch_size with a matching byte cap is a supported configuration");
    config
}

fn open_client() -> (Arc<PulseDB>, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let db = Arc::new(PulseDB::open(dir.path().join("client.db"), Config::default()).unwrap());
    (db, dir)
}

/// Class Q: a FULL poll page must report `has_more`, even when every event it
/// polled turned into a change.
///
/// With `batch_size` at or above the poll page the two caps coincide —
/// `events.len() == changes.len() == 1000` — so a `has_more` computed from the
/// batch alone reads "exhausted" while the WAL still holds events. `initial_sync`
/// breaks on `!has_more` and returns `Ok(())`, which under the contract this PR
/// introduced means the catch-up COMPLETED. It had not: the changes past the
/// page were never fetched.
#[tokio::test]
async fn a_full_poll_page_reports_more_even_when_every_event_yielded_a_change() {
    let server = start_test_server().await;
    let ids = fill_wal_with_collectives(&server.db, SERVER_PULL_PAGE_EVENTS + 5);
    let batch_size = 2 * SERVER_PULL_PAGE_EVENTS;

    let response = pull_page(&server.server, 0, batch_size, None);
    assert_eq!(
        response.changes.len(),
        SERVER_PULL_PAGE_EVENTS,
        "the poll page, not batch_size, is what bounds this batch"
    );
    assert!(
        response.has_more,
        "a full poll page with events behind it must report more — the batch \
         count cannot tell exhaustion from saturation when the two caps coincide"
    );

    // The consequence, end to end: `Ok(())` from `initial_sync` has to mean the
    // catch-up completed.
    let (db_client, _dir_client) = open_client();
    let mut manager = SyncManager::new(
        Arc::clone(&db_client),
        Box::new(HttpSyncTransport::new(&server.base_url)),
        wide_batch_config(batch_size),
    );
    manager
        .initial_sync(None)
        .await
        .expect("every change applies, so the catch-up completes");

    let missing = ids
        .iter()
        .filter(|id| db_client.get_collective(**id).unwrap().is_none())
        .count();
    assert_eq!(
        missing,
        0,
        "`Ok(())` means the catch-up completed, but {missing} of {} collectives \
         never arrived",
        ids.len()
    );
}

/// The other side of the rule: a WAL that ends EXACTLY on the poll limit.
///
/// One poll cannot tell that page from a saturated one, so the server reports
/// `has_more` conservatively. This test pins that over-reporting as harmless —
/// the follow-up pull comes back empty with `has_more: false` and an unadvanced
/// cursor, which `initial_sync`'s existing guard reads as the ordinary
/// caught-up end and returns `Ok(())`.
#[tokio::test]
async fn a_page_that_exactly_exhausts_the_wal_over_reports_and_still_completes() {
    let server = start_test_server().await;
    let ids = fill_wal_with_collectives(&server.db, SERVER_PULL_PAGE_EVENTS);
    assert_eq!(
        server.db.get_current_sequence().unwrap(),
        SERVER_PULL_PAGE_EVENTS as u64,
        "the fixture must leave the WAL ending exactly on the poll limit"
    );
    let batch_size = 2 * SERVER_PULL_PAGE_EVENTS;

    let first = pull_page(&server.server, 0, batch_size, None);
    assert_eq!(first.changes.len(), SERVER_PULL_PAGE_EVENTS);
    assert!(
        first.has_more,
        "a full page is reported as possibly-more: nothing in this poll could \
         have told the server the WAL ended on the limit"
    );

    let second = pull_page(&server.server, first.new_cursor.sequence, batch_size, None);
    assert!(
        second.changes.is_empty(),
        "there was nothing beyond the page after all"
    );
    assert!(
        !second.has_more,
        "the short page is the evidence of exhaustion, and this one is empty"
    );
    assert_eq!(
        second.new_cursor.sequence, first.new_cursor.sequence,
        "an empty batch leaves the cursor exactly where it was"
    );

    let (db_client, _dir_client) = open_client();
    let mut manager = SyncManager::new(
        Arc::clone(&db_client),
        Box::new(HttpSyncTransport::new(&server.base_url)),
        wide_batch_config(batch_size),
    );
    manager
        .initial_sync(None)
        .await
        .expect("the conservative has_more costs one empty pull, not the completion");

    let missing = ids
        .iter()
        .filter(|id| db_client.get_collective(**id).unwrap().is_none())
        .count();
    assert_eq!(missing, 0, "{missing} collectives never arrived");
}

/// A page SHORT of the poll limit is untouched by the saturation rule: the
/// default `batch_size` still reports exhaustion when it drains the page, and a
/// `batch_size` below the page still reports more.
#[tokio::test]
async fn a_short_poll_page_reports_exactly_what_it_did_before() {
    let server = start_test_server().await;
    let ids = fill_wal_with_collectives(&server.db, 5);
    assert!(ids.len() < SERVER_PULL_PAGE_EVENTS);

    let drained = pull_page(&server.server, 0, SyncConfig::default().batch_size, None);
    assert_eq!(drained.changes.len(), 5);
    assert!(
        !drained.has_more,
        "a short page with every event emitted is the exhausted WAL"
    );
    assert_eq!(drained.new_cursor.sequence, 5);

    let capped = pull_page(&server.server, 0, 2, None);
    assert_eq!(capped.changes.len(), 2, "batch_size caps this batch");
    assert!(
        capped.has_more,
        "the page held more events than batch_size emitted"
    );
    assert_eq!(capped.new_cursor.sequence, 2);

    let rest = pull_page(&server.server, capped.new_cursor.sequence, 2, None);
    assert_eq!(rest.changes.len(), 2);
    assert!(rest.has_more);
    let last = pull_page(&server.server, rest.new_cursor.sequence, 2, None);
    assert_eq!(last.changes.len(), 1);
    assert!(
        !last.has_more,
        "the remainder drains the page and reports exhaustion"
    );
}

/// The behaviour issue #90 tracks, at the real server: a page where the
/// `collectives` filter removed every event returns an empty batch with an
/// UNADVANCED cursor and `has_more: true`, and `initial_sync` turns that stall
/// into `CatchUpIncomplete` rather than spinning on the identical request.
///
/// The saturation rule must not disturb this — a short page that emitted fewer
/// changes than it polled still reports more.
#[tokio::test]
async fn a_fully_filtered_page_still_stalls_the_catch_up() {
    let server = start_test_server().await;
    fill_wal_with_collectives(&server.db, 3);
    let unrelated = CollectiveId::new();

    let response = pull_page(&server.server, 0, 500, Some(vec![unrelated]));
    assert!(
        response.changes.is_empty(),
        "every event belongs to a collective the filter excludes"
    );
    assert!(
        response.has_more,
        "the page held events this batch did not emit"
    );
    assert_eq!(
        response.new_cursor.sequence, 0,
        "an empty batch does not advance the cursor"
    );

    let (db_client, _dir_client) = open_client();
    let mut manager = SyncManager::new(
        Arc::clone(&db_client),
        Box::new(HttpSyncTransport::new(&server.base_url)),
        SyncConfig {
            direction: SyncDirection::PullOnly,
            collectives: Some(vec![unrelated]),
            ..SyncConfig::default()
        },
    );

    let error = manager
        .initial_sync(None)
        .await
        .expect_err("a peer that promises more and will not advance has not caught us up");
    assert!(
        error.is_catch_up_incomplete(),
        "the stall must surface as the typed catch-up error, got: {error}"
    );
    assert!(
        error.to_string().contains("did not advance the cursor"),
        "the error must name what stopped it, got: {error}"
    );
}
