//! HTTP sync transport implementation.
//!
//! [`HttpSyncTransport`] implements [`SyncTransport`] using `reqwest` for
//! HTTP communication. Uses postcard serialization for compact payloads.
//!
//! The handshake request/response additionally carry a serializer-independent
//! 3-byte wire preamble (validated by raw byte-slice before deserialize) so a
//! cross-version peer fails loud in BOTH directions; push/pull bodies carry no
//! preamble (they are reached only after the handshake pinned the version).
//!
//! # Example
//!
//! ```rust,ignore
//! use pulsedb::sync::transport_http::HttpSyncTransport;
//!
//! let transport = HttpSyncTransport::new("http://server:3000");
//! // or with authentication:
//! let transport = HttpSyncTransport::with_auth("https://server:3000", "my-secret-token");
//! ```

use async_trait::async_trait;
use reqwest::Client;
use tracing::debug;

use super::error::SyncError;
use super::transport::SyncTransport;
use super::types::{
    HandshakeRequest, HandshakeResponse, PullRequest, PullResponse, PushResponse, SyncChange,
};
use super::{read_wire_preamble, write_wire_preamble};

/// HTTP-based sync transport using reqwest.
///
/// Communicates with a remote PulseDB sync server over HTTP using
/// postcard-serialized request/response bodies (the handshake additionally
/// carries a 3-byte wire preamble in both directions).
///
/// # Endpoints
///
/// | Method | Path | Request | Response |
/// |--------|------|---------|----------|
/// | POST | `/sync/handshake` | `HandshakeRequest` | `HandshakeResponse` |
/// | POST | `/sync/push` | `Vec<SyncChange>` | `PushResponse` |
/// | POST | `/sync/pull` | `PullRequest` | `PullResponse` |
/// | GET | `/sync/health` | (none) | 200 OK |
pub struct HttpSyncTransport {
    client: Client,
    base_url: String,
    auth_token: Option<String>,
}

impl HttpSyncTransport {
    /// Creates a new HTTP transport pointing at the given base URL.
    ///
    /// The URL should not include a trailing slash.
    /// Example: `"http://localhost:3000"` or `"https://api.example.com"`
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.into(),
            auth_token: None,
        }
    }

    /// Creates a new HTTP transport with Bearer token authentication.
    ///
    /// The token is sent as `Authorization: Bearer {token}` on every request.
    pub fn with_auth(base_url: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.into(),
            auth_token: Some(token.into()),
        }
    }

    /// Sends a POST with a raw byte body and returns the raw response bytes.
    ///
    /// This is the framing-agnostic transport leg: it knows nothing about
    /// postcard or the wire preamble. The handshake call layers preamble
    /// framing/validation on top; push/pull layer plain postcard on top.
    async fn post_raw(&self, path: &str, body: Vec<u8>) -> Result<Vec<u8>, SyncError> {
        let url = format!("{}{}", self.base_url, path);

        let mut req = self
            .client
            .post(&url)
            .header("Content-Type", "application/octet-stream")
            .body(body);

        if let Some(ref token) = self.auth_token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }

        let response = req.send().await.map_err(|e| {
            if e.is_timeout() {
                SyncError::Timeout
            } else if e.is_connect() {
                SyncError::ConnectionLost
            } else {
                SyncError::transport(e.to_string())
            }
        })?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_else(|_| "unknown".into());
            return Err(if status.is_client_error() {
                SyncError::invalid_payload(format!("HTTP {}: {}", status, body_text))
            } else {
                SyncError::transport(format!("HTTP {}: {}", status, body_text))
            });
        }

        response
            .bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| SyncError::transport(format!("Failed to read response body: {}", e)))
    }

    /// Sends a POST with a postcard body and deserializes the postcard response.
    ///
    /// Used for push/pull, which carry NO wire preamble (they are reached only
    /// after the handshake pinned the version). The handshake does NOT use this
    /// path — it is specialized in [`HttpSyncTransport::handshake`] to frame and
    /// validate the preamble in both directions.
    async fn post_body<Req, Resp>(&self, path: &str, request: &Req) -> Result<Resp, SyncError>
    where
        Req: serde::Serialize,
        Resp: serde::de::DeserializeOwned,
    {
        let body =
            postcard::to_allocvec(request).map_err(|e| SyncError::serialization(e.to_string()))?;
        let response_bytes = self.post_raw(path, body).await?;
        postcard::from_bytes(&response_bytes)
            .map_err(|e| SyncError::serialization(format!("Response deserialization: {}", e)))
    }
}

#[async_trait]
impl SyncTransport for HttpSyncTransport {
    async fn handshake(&self, request: HandshakeRequest) -> Result<HandshakeResponse, SyncError> {
        debug!(url = %self.base_url, "HTTP sync handshake");
        // Handshake — and ONLY the handshake — carries the wire preamble in
        // both directions. Encode the body, frame it with the preamble, then
        // on the response validate the preamble by raw byte-slice BEFORE
        // deserializing — so a cross-version server fails loud here too.
        let encoded = postcard::to_allocvec(&request)
            .map_err(|e| SyncError::serialization(e.to_string()))?;
        let framed = write_wire_preamble(&encoded);
        let response_bytes = self.post_raw("/sync/handshake", framed).await?;
        let payload = read_wire_preamble(&response_bytes)?;
        postcard::from_bytes(payload)
            .map_err(|e| SyncError::serialization(format!("Response deserialization: {}", e)))
    }

    async fn push_changes(&self, changes: Vec<SyncChange>) -> Result<PushResponse, SyncError> {
        let count = changes.len();
        debug!(count, "HTTP sync push");
        self.post_body("/sync/push", &changes).await
    }

    async fn pull_changes(&self, request: PullRequest) -> Result<PullResponse, SyncError> {
        debug!("HTTP sync pull");
        self.post_body("/sync/pull", &request).await
    }

    async fn health_check(&self) -> Result<(), SyncError> {
        let url = format!("{}/sync/health", self.base_url);

        let mut req = self.client.get(&url);
        if let Some(ref token) = self.auth_token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }

        let response = req.send().await.map_err(|e| {
            if e.is_timeout() {
                SyncError::Timeout
            } else if e.is_connect() {
                SyncError::ConnectionLost
            } else {
                SyncError::transport(e.to_string())
            }
        })?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(SyncError::transport(format!(
                "Health check failed: HTTP {}",
                response.status()
            )))
        }
    }
}

// HttpSyncTransport is Send + Sync (reqwest::Client is Send + Sync)
