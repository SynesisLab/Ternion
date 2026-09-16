//! Command error type returned to the frontend as `{ code, message }`.

use serde::ser::SerializeStruct;

#[derive(Debug, thiserror::Error)]
#[error("{code}: {message}")]
pub struct CmdError {
    pub code: String,
    pub message: String,
}

impl CmdError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new("internal", message)
    }

    pub fn endpoint_unreachable(message: impl Into<String>) -> Self {
        Self::new("endpoint_unreachable", message)
    }

    pub fn stream_active() -> Self {
        Self::new(
            "stream_active",
            "A response is already streaming in this conversation",
        )
    }
}

impl serde::Serialize for CmdError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("CmdError", 2)?;
        s.serialize_field("code", &self.code)?;
        s.serialize_field("message", &self.message)?;
        s.end()
    }
}

impl From<rusqlite::Error> for CmdError {
    fn from(e: rusqlite::Error) -> Self {
        log::error!("database error: {e}");
        Self::new("db_error", e.to_string())
    }
}

impl From<reqwest::Error> for CmdError {
    fn from(e: reqwest::Error) -> Self {
        log::error!("http error: {e}");
        Self::endpoint_unreachable(e.to_string())
    }
}