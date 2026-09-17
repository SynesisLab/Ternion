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

/// Split a model reference into its endpoint dimension (design §5.1: roles
/// bind to `(endpoint, model)` pairs). Bare `"gemma3:4b"` means the built-in
/// local Ollama (keeps every pre-M3 pin/role/settings value valid);
/// `"llama3@ep_abc"` targets that profile.
pub fn parse_model_ref(reference: &str) -> (Option<String>, String) {
    match reference.rsplit_once('@') {
        Some((model, endpoint)) if !model.is_empty() && !endpoint.is_empty() => {
            (Some(endpoint.to_string()), model.to_string())
        }
        _ => (None, reference.to_string()),
    }
}

/// The stored/display form of a model on an endpoint: bare for the built-in
/// local Ollama, `model@endpoint` everywhere else.
pub fn qualified_model_ref(endpoint_id: &str, model: &str) -> String {
    if endpoint_id == crate::state::DEFAULT_ENDPOINT {
        model.to_string()
    } else {
        format!("{model}@{endpoint_id}")
    }
}

/// Registry from the built-in local Ollama plus every enabled profile (§5.1).
/// A profile whose keyring read fails still gets built — the request path
/// surfaces a clearer 401 than a silently missing endpoint.
pub fn build_providers(
    profiles: &[crate::types::EndpointProfile],
    ollama_base_url: &str,
    http: reqwest::Client,
) -> HashMap<String, Arc<dyn Provider>> {
    let mut map: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    let adapter = Arc::new(ollama::OllamaAdapter::new(
        crate::state::DEFAULT_ENDPOINT,
        ollama_base_url,
        http.clone(),
    ));
    map.insert(adapter.id().to_string(), adapter);

    for profile in profiles.iter().filter(|p| p.enabled) {
        let api_key = profile.api_key_ref.as_deref().and_then(|key_ref| {
            match secrets::read_api_key(key_ref) {
                Ok(key) => key,
                Err(e) => {
                    log::warn!("api key read for {} failed: {e}", profile.id);
                    None
                }
            }
        });
        let headers: Vec<(String, String)> = profile
            .headers
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let adapter: Arc<dyn Provider> = match profile.kind.as_str() {
            "openai" => Arc::new(openai::OpenAiAdapter::new(
                &profile.id,
                &profile.base_url,
                api_key,
                headers,
                http.clone(),
            )),
            // Default (and "ollama"): the native Ollama API.
            _ => Arc::new(ollama::OllamaAdapter::new(&profile.id, &profile.base_url, http.clone())),
        };
        map.insert(adapter.id().to_string(), adapter);
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_refs_parse_across_endpoints() {
        // Bare = built-in (back-compat with every pre-M3 reference).
        assert_eq!(parse_model_ref("gemma3:4b"), (None, "gemma3:4b".to_string()));
        // Qualified = that endpoint.
        assert_eq!(
            parse_model_ref("llama3@ep_abc"),
            (Some("ep_abc".to_string()), "llama3".to_string())
        );
        // Model names legitimately contain ':' — the '@' split keeps them.
        assert_eq!(
            parse_model_ref("gemma3:4b@ep_local_ollama"),
            (Some("ep_local_ollama".to_string()), "gemma3:4b".to_string())
        );
        // Malformed refs degrade to bare (never panic, never lose the model).
        assert_eq!(parse_model_ref("@ep_x"), (None, "@ep_x".to_string()));
        assert_eq!(parse_model_ref("model@"), (None, "model@".to_string()));
    }

    #[test]
    fn qualification_stays_bare_for_the_builtin() {
        assert_eq!(
            qualified_model_ref(crate::state::DEFAULT_ENDPOINT, "gemma3:4b"),
            "gemma3:4b"
        );
        assert_eq!(
            qualified_model_ref("ep_remote", "llama3"),
            "llama3@ep_remote"
        );
    }

    #[test]
    fn build_providers_from_profiles() {
        let http = reqwest::Client::new();
        let profiles = vec![
            crate::types::EndpointProfile {
                id: "ep_remote_ollama".into(),
                kind: "ollama".into(),
                name: "LAN Ollama".into(),
                base_url: "http://192.168.1.30:11434".into(),
                api_key_ref: None,
                headers: Default::default(),
                enabled: true,
                notes: None,
            },
            crate::types::EndpointProfile {
                id: "ep_disabled".into(),
                kind: "openai".into(),
                name: "Off".into(),
                base_url: "http://127.0.0.1:1234".into(),
                api_key_ref: None,
                headers: Default::default(),
                enabled: false,
                notes: None,
            },
        ];
        let map = build_providers(&profiles, "http://127.0.0.1:11434", http);
        // Built-in + the enabled profile; disabled excluded.
        assert!(map.contains_key(crate::state::DEFAULT_ENDPOINT));
        assert!(map.contains_key("ep_remote_ollama"));
        assert!(!map.contains_key("ep_disabled"));
        assert_eq!(map.len(), 2);

        // Kinds survive construction.
        let remote = map.get("ep_remote_ollama").unwrap();
        assert_eq!(remote.kind(), EndpointKind::Ollama);
        let builtin = map.get(crate::state::DEFAULT_ENDPOINT).unwrap();
        assert_eq!(builtin.kind(), EndpointKind::Ollama);
    }

    #[test]
    fn openai_profile_builds_an_openai_adapter() {
        let http = reqwest::Client::new();
        let profiles = vec![crate::types::EndpointProfile {
            id: "ep_lmstudio".into(),
            kind: "openai".into(),
            name: "LM Studio".into(),
            base_url: "http://127.0.0.1:1234".into(),
            api_key_ref: None,
            headers: Default::default(),
            enabled: true,
            notes: None,
        }];
        let map = build_providers(&profiles, "http://127.0.0.1:11434", http);
        let adapter = map.get("ep_lmstudio").unwrap();
        assert_eq!(adapter.kind(), EndpointKind::OpenAiCompat);
    }
}