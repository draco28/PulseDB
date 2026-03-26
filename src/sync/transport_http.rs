//! HTTP sync transport implementation.
//!
//! [`HttpSyncTransport`] implements [`SyncTransport`] using `reqwest` for
//! HTTP communication. Uses bincode serialization for compact payloads.
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

/// HTTP-based sync transport using reqwest.
///
/// Communicates with a remote PulseDB sync server over HTTP using
/// bincode-serialized request/response bodies.
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

    /// Sends a POST request with a bincode body and deserializes the response.
    async fn post_bincode<Req, Resp>(&self, path: &str, request: &Req) -> Result<Resp, SyncError>
    where
        Req: serde::Serialize,
        Resp: serde::de::DeserializeOwned,
    {
        let url = format!("{}{}", self.base_url, path);
        let body =
            bincode::serialize(request).map_err(|e| SyncError::serialization(e.to_string()))?;

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

        let response_bytes = response
            .bytes()
            .await
            .map_err(|e| SyncError::transport(format!("Failed to read response body: {}", e)))?;

        bincode::deserialize(&response_bytes)
            .map_err(|e| SyncError::serialization(format!("Response deserialization: {}", e)))
    }
}

#[async_trait]
impl SyncTransport for HttpSyncTransport {
    async fn handshake(&self, request: HandshakeRequest) -> Result<HandshakeResponse, SyncError> {
        debug!(url = %self.base_url, "HTTP sync handshake");
        self.post_bincode("/sync/handshake", &request).await
    }

    async fn push_changes(&self, changes: Vec<SyncChange>) -> Result<PushResponse, SyncError> {
        let count = changes.len();
        debug!(count, "HTTP sync push");
        self.post_bincode("/sync/push", &changes).await
    }

    async fn pull_changes(&self, request: PullRequest) -> Result<PullResponse, SyncError> {
        debug!("HTTP sync pull");
        self.post_bincode("/sync/pull", &request).await
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
