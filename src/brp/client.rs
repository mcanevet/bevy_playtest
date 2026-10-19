//! An asynchronous JSON-RPC 2.0 over HTTP client for the Bevy Remote Protocol.
//!
//! CLONED from Bevy upstream (`bevy_remote::client`, 0.20-dev @ c9c21c2b7)
//! with two adaptations:
//! 1. Uses `bevy_playtest`'s own wire types instead of `bevy_remote`'s,
//!    so this crate stays Bevy-free except for `bevy_tasks`/`bevy_platform`.
//! 2. `spawn_call` detaches the connection-driver task on a plain
//!    `TaskPool` (avoids `IoTaskPool::get_or_init` requiring an app).
//!
//! When Bevy releases the official client, swap this module for the upstream
//! one; the public API (`BrpClient::new/localhost/call`, `BrpClientError`)
//! is kept identical for that purpose.
//!
//! License note: upstream is dual MIT OR Apache-2.0; copying is permitted
//! and the LICENSE files of this repository carry the notices.

use crate::brp::wire::{BrpPayload, BrpRequest, BrpResponse};
use core::{
    net::SocketAddr,
    sync::atomic::{AtomicU64, Ordering},
};
use std::net::{TcpStream, ToSocketAddrs as _};
use std::sync::Arc;

use async_io::Async;
use http_body_util::{BodyExt as _, Full};
use hyper::{
    body::Bytes,
    client::conn::http1,
    header::{CONTENT_TYPE, HOST},
    Request, StatusCode,
};
use serde_json::Value;
use smol_hyper::rt::FuturesIo;

/// A client for one Bevy Remote Protocol HTTP endpoint.
#[derive(Clone, Debug)]
pub struct BrpClient {
    host: String,
    port: u16,
    path: String,
    request_id: Arc<AtomicU64>,
}

impl BrpClient {
    /// Creates a client targeting `host:port` at the root path.
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            path: "/".to_string(),
            request_id: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Creates a client targeting `127.0.0.1:port`.
    pub fn localhost(port: u16) -> Self {
        Self::new("127.0.0.1", port)
    }

    /// Sets the request path, which defaults to `/`.
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = path.into();
        self
    }

    /// Returns the full URL this client posts to.
    pub fn url(&self) -> String {
        format!("http://{}:{}{}", self.host, self.port, self.path)
    }

    /// Sends one request and awaits the JSON-RPC result. Watching methods
    /// return a stream that this method does not decode.
    pub async fn call(&self, method: &str, params: Option<Value>) -> Result<Value, BrpClientError> {
        let request = BrpRequest {
            method: method.to_string(),
            id: Some(Value::from(self.request_id.fetch_add(1, Ordering::Relaxed))),
            params,
        };
        let body = serde_json::to_vec(&request)?;

        let address = self.resolve()?;
        let stream = Async::<TcpStream>::connect(address).await?;
        let (mut sender, connection) = http1::handshake(FuturesIo::new(stream)).await?;
        std::thread::spawn(move || {
            // drive the hyper connection to completion on a helper thread;
            // futures_lite::future::block_on avoids pulling a full runtime
            let _ = futures_lite_block_on(connection);
        });

        let http_request = Request::post(&self.path)
            .header(HOST, format!("{}:{}", self.host, self.port))
            .header(CONTENT_TYPE, "application/json")
            .body(Full::new(Bytes::from(body)))
            .map_err(|error| BrpClientError::InvalidResponse(error.to_string()))?;

        let http_response = sender.send_request(http_request).await?;
        let status = http_response.status();
        let bytes = http_response.into_body().collect().await?.to_bytes();

        if status != StatusCode::OK {
            return Err(BrpClientError::InvalidResponse(format!(
                "server responded with status {status}"
            )));
        }

        let response: BrpResponse = serde_json::from_slice(&bytes)?;
        match response.payload {
            BrpPayload::Result(value) => Ok(value),
            BrpPayload::Error(error) => Err(BrpClientError::Remote(error)),
        }
    }

    /// Sends the request and returns the response as raw bytes, bypassing
    /// response-envelope decoding. Used for probing servers whose payloads
    /// do not strictly follow the envelope.
    pub async fn call_raw(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<Bytes, BrpClientError> {
        let request = BrpRequest {
            method: method.to_string(),
            id: Some(Value::from(self.request_id.fetch_add(1, Ordering::Relaxed))),
            params,
        };
        let body = serde_json::to_vec(&request)?;
        let address = self.resolve()?;
        let stream = Async::<TcpStream>::connect(address).await?;
        let (mut sender, connection) = http1::handshake(FuturesIo::new(stream)).await?;
        std::thread::spawn(move || {
            let _ = futures_lite_block_on(connection);
        });

        let http_request = Request::post(&self.path)
            .header(HOST, format!("{}:{}", self.host, self.port))
            .header(CONTENT_TYPE, "application/json")
            .body(Full::new(Bytes::from(body)))
            .map_err(|error| BrpClientError::InvalidResponse(error.to_string()))?;

        let http_response = sender.send_request(http_request).await?;
        let status = http_response.status();
        let bytes = http_response.into_body().collect().await?.to_bytes();
        if status != StatusCode::OK {
            return Err(BrpClientError::InvalidResponse(format!(
                "server responded with status {status}"
            )));
        }
        Ok(bytes)
    }

    fn resolve(&self) -> Result<SocketAddr, BrpClientError> {
        (self.host.as_str(), self.port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| {
                BrpClientError::InvalidResponse(format!("no address for host {}", self.host))
            })
    }
}

/// Block on a future without a full async runtime.
///
/// Uses a busy-spin executor: adequate for driving one hyper connection
/// thread whose sole job is to flush the socket.
fn futures_lite_block_on<F: std::future::Future>(fut: F) -> F::Output {
    futures_lite::future::block_on(fut)
}

/// Errors from a BRP client call.
#[derive(Debug)]
pub enum BrpClientError {
    /// The connection could not be established or was lost.
    Io(std::io::Error),
    /// The HTTP exchange failed.
    Http(hyper::Error),
    /// The request or response could not be (de)serialized.
    Json(serde_json::Error),
    /// The remote app returned a JSON-RPC error.
    Remote(crate::brp::wire::BrpError),
    /// The response was not a well-formed BRP response.
    InvalidResponse(String),
}

impl From<std::io::Error> for BrpClientError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<hyper::Error> for BrpClientError {
    fn from(value: hyper::Error) -> Self {
        Self::Http(value)
    }
}

impl From<serde_json::Error> for BrpClientError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

impl core::fmt::Display for BrpClientError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "I/O error: {error}"),
            Self::Http(error) => write!(f, "HTTP error: {error}"),
            Self::Json(error) => write!(f, "JSON error: {error}"),
            Self::Remote(error) => {
                write!(f, "remote error {}: {}", error.code, error.message)
            }
            Self::InvalidResponse(message) => write!(f, "invalid response: {message}"),
        }
    }
}

impl core::error::Error for BrpClientError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Http(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::Remote(_) | Self::InvalidResponse(_) => None,
        }
    }
}
