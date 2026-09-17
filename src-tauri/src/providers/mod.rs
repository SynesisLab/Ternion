//! Provider abstraction (design §5). Every endpoint kind compiles to the same
//! normalized stream, which is what makes swapping Scout/Titan mid-thread
//! trivial later. M0 ships exactly one provider: local Ollama.

pub mod ndjson;
pub mod ollama;
pub mod openai;
pub mod probe;
pub mod secrets;
#[cfg(test)]
pub mod test_support;

use std::collections::HashMap;
use std::sync::Arc;

use futures::future::BoxFuture;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    error::CmdError,
    types::{ChatRequest, ModelInfo, StreamEvent},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointKind {
    Ollama,
    /// OpenAI-compatible endpoints (M3).
    OpenAiCompat,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("endpoint unreachable: {0}")]
    EndpointUnreachable(String),
    #[error("http {0}: {1}")]
    Http(u16, String),
    #[error("malformed response: {0}")]
    Malformed(String),
}

impl From<ProviderError> for CmdError {
    fn from(e: ProviderError) -> Self {
        match e {
            ProviderError::EndpointUnreachable(m) => CmdError::endpoint_unreachable(m),
            ProviderError::Http(code, body) => CmdError::new("http_error", format!("HTTP {code}: {body}")),
            ProviderError::Malformed(m) => CmdError::internal(m),
        }
    }
}

pub trait Provider: Send + Sync {
    fn id(&self) -> &str;
    fn kind(&self) -> EndpointKind;
    /// Discover models (design §5.2/§5.3 discovery row).
    fn list_models(&self) -> BoxFuture<'_, Result<Vec<ModelInfo>, ProviderError>>;
    /// Spawned by the orchestrator; writes normalized StreamEvents into
    /// `events` and returns when the stream ends. Honors `cancel` by dropping
    /// the HTTP response so the server frees its generation slot.
    fn chat(
        &self,
        req: ChatRequest,
        events: mpsc::Sender<StreamEvent>,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, ()>;
}

/// M0 registry: a single Ollama provider at the configured base URL.
/// M3 extends this to user-managed endpoint profiles.
pub fn build_providers(
    ollama_base_url: &str,
    http: reqwest::Client,
) -> HashMap<String, Arc<dyn Provider>> {
    let mut map: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    let adapter = Arc::new(ollama::OllamaAdapter::new(
        "ep_local_ollama",
        ollama_base_url,
        http,
    ));
    map.insert(adapter.id().to_string(), adapter);
    map
}