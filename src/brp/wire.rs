//! BRP wire types: the JSON-RPC 2.0 envelopes spoken by the Bevy Remote
//! Protocol. Extracted from Bevy's `bevy_remote` crate (0.20-dev, commit
//! c9c21c2b7) so that `bevy_playtest` can speak BRP without depending
//! on Bevy. When Bevy ships a released `BrpClient`, this module can be
//! swapped for the upstream types behind the same public API.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A single BRP request (JSON-RPC 2.0).
#[derive(Debug, Clone)]
pub struct BrpRequest {
    /// The method to call, e.g. `world.query`.
    pub method: String,
    /// The request id, echoed back in the response.
    pub id: Option<Value>,
    /// Method-specific parameters.
    pub params: Option<Value>,
}

impl Serialize for BrpRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("jsonrpc", "2.0")?;
        map.serialize_entry("method", &self.method)?;
        if self.id.is_some() {
            map.serialize_entry("id", &self.id)?;
        }
        if self.params.is_some() {
            map.serialize_entry("params", &self.params)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for BrpRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let obj = value
            .as_object()
            .ok_or_else(|| serde::de::Error::custom("BRP request must be a JSON object"))?;
        match obj.get("jsonrpc").and_then(Value::as_str) {
            Some("2.0") => {}
            _ => {
                return Err(serde::de::Error::custom(
                    "BRP request must contain \"jsonrpc\":\"2.0\"",
                ))
            }
        }
        let method = obj
            .get("method")
            .and_then(Value::as_str)
            .ok_or_else(|| serde::de::Error::custom("BRP request must contain a method"))?
            .to_string();
        let id = obj.get("id").cloned();
        let params = obj.get("params").cloned();
        Ok(Self { method, id, params })
    }
}

/// A BRP response payload: either a result or an error.
#[derive(Debug, Clone, PartialEq)]
pub enum BrpPayload {
    /// Successful result.
    Result(Value),
    /// JSON-RPC error.
    Error(BrpError),
}

/// A BRP response (JSON-RPC 2.0).
#[derive(Debug, Clone)]
pub struct BrpResponse {
    /// The id of the originating request.
    pub id: Option<Value>,
    /// The payload.
    pub payload: BrpPayload,
}

impl Serialize for BrpResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("jsonrpc", "2.0")?;
        if self.id.is_some() {
            map.serialize_entry("id", &self.id)?;
        }
        match &self.payload {
            BrpPayload::Result(value) => map.serialize_entry("result", value)?,
            BrpPayload::Error(error) => map.serialize_entry("error", error)?,
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for BrpResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let obj = value
            .as_object()
            .ok_or_else(|| serde::de::Error::custom("BRP response must be a JSON object"))?;
        match obj.get("jsonrpc").and_then(Value::as_str) {
            Some("2.0") => {}
            _ => {
                return Err(serde::de::Error::custom(
                    "BRP response must contain \"jsonrpc\":\"2.0\"",
                ))
            }
        }
        let id = obj.get("id").cloned();
        let payload = if let Some(result) = obj.get("result") {
            BrpPayload::Result(result.clone())
        } else if let Some(error) = obj.get("error") {
            BrpPayload::Error(BrpError::deserialize(error).map_err(serde::de::Error::custom)?)
        } else {
            return Err(serde::de::Error::custom(
                "BRP response must contain either a result or an error",
            ));
        };
        Ok(Self { id, payload })
    }
}

/// A JSON-RPC error returned by a BRP server.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct BrpError {
    /// The general error category.
    pub code: i16,
    /// Human-readable description.
    pub message: String,
    /// Optional extra data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// Standard BRP error codes (mirrors `bevy_remote::error_codes`).
pub mod error_codes {
    /// The JSON-RPC parse failed.
    pub const PARSE_ERROR: i16 = -32700;
    /// The request was not a valid JSON-RPC request.
    pub const INVALID_REQUEST: i16 = -32600;
    /// The method does not exist.
    pub const METHOD_NOT_FOUND: i16 = -32601;
    /// The method parameters were invalid.
    pub const INVALID_PARAMS: i16 = -32602;
    /// Internal server error.
    pub const INTERNAL_ERROR: i16 = -32603;
    /// The requested entity was not found.
    pub const ENTITY_NOT_FOUND: i16 = -23401;
    /// The requested component type is unknown.
    pub const COMPONENT_TYPE_UNKNOWN: i16 = -23402;
    /// The component is not present on the entity.
    pub const COMPONENT_NOT_PRESENT: i16 = -23403;
    /// The requested resource was not found.
    pub const RESOURCE_NOT_FOUND: i16 = -23404;
}
