//! Native Ollama adapter (design §5.2): `/api/tags` discovery and
//! `/api/chat` NDJSON streaming.

use std::sync::Arc;

use futures::future::BoxFuture;
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{ndjson::LineSplitter, Provider, ProviderError};
use crate::{
    types::{ChatContent, ChatMessage, ChatRequest, ModelInfo, StreamEvent},
};

pub struct OllamaAdapter {
    endpoint_id: String,
    base_url: String,
    http: reqwest::Client,
}

impl OllamaAdapter {
    pub fn new(endpoint_id: &str, base_url: &str, http: reqwest::Client) -> Self {
        Self {
            endpoint_id: endpoint_id.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
            http,
        }
    }

    async fn list_models_impl(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let url = format!("{}/api/tags", self.base_url);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| ProviderError::EndpointUnreachable(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(ProviderError::Http(
                resp.status().as_u16(),
                resp.text().await.unwrap_or_default(),
            ));
        }
        let value: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::Malformed(e.to_string()))?;
        parse_tags(&value).map_err(|e| ProviderError::Malformed(e.to_string()))
    }
}

impl Provider for OllamaAdapter {
    fn id(&self) -> &str {
        &self.endpoint_id
    }

    fn kind(&self) -> super::EndpointKind {
        super::EndpointKind::Ollama
    }

    fn list_models(&self) -> BoxFuture<'_, Result<Vec<ModelInfo>, ProviderError>> {
        Box::pin(self.list_models_impl())
    }

    fn chat(
        &self,
        _req: ChatRequest,
        _events: mpsc::Sender<StreamEvent>,
        _cancel: CancellationToken,
    ) -> BoxFuture<'_, ()> {
        // Wired to the NDJSON parser in step 7.
        Box::pin(async { log::debug!("ollama chat: wired in step 7") })
    }
}

// ---------------------------------------------------------------------------
// /api/tags mapping
// ---------------------------------------------------------------------------

#[derive(Deserialize, Debug)]
struct TagsResponse {
    #[serde(default)]
    models: Vec<TagModel>,
}

#[derive(Deserialize, Debug)]
struct TagModel {
    #[serde(default)]
    name: Option<String>,
    /// Some Ollama versions emit `model` instead of `name`; fall back to it.
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default)]
    context_length: Option<u32>,
    #[serde(default)]
    details: Option<TagDetails>,
}

#[derive(Deserialize, Debug, Default)]
struct TagDetails {
    #[serde(default)]
    family: Option<String>,
    #[serde(default)]
    parameter_size: Option<String>,
    #[serde(default)]
    quantization_level: Option<String>,
}

fn parse_tags(value: &serde_json::Value) -> Result<Vec<ModelInfo>, String> {
    let resp: TagsResponse =
        serde_json::from_value(value.clone()).map_err(|e| format!("api/tags: {e}"))?;
    Ok(resp
        .models
        .into_iter()
        .filter_map(|m| {
            // `name` is the canonical tag; `model` is the fallback spelling.
            let name = m.name.or(m.model)?;
            let details = m.details.unwrap_or_default();
            Some(ModelInfo {
                id: name.clone(),
                display_name: name,
                size_bytes: m.size,
                parameter_size: details.parameter_size,
                quantization_level: details.quantization_level,
                family: details.family,
                context_length: m.context_length,
                capabilities: m.capabilities,
            })
        })
        .collect())
}

// ---------------------------------------------------------------------------
// /api/chat request body (pure, unit-tested)
// ---------------------------------------------------------------------------

pub(crate) fn build_chat_body(req: &ChatRequest) -> serde_json::Value {
    let mut options = serde_json::Map::new();
    if let Some(t) = req.params.temperature {
        options.insert("temperature".into(), serde_json::json!(t));
    }
    if let Some(p) = req.params.top_p {
        options.insert("top_p".into(), serde_json::json!(p));
    }
    if let Some(c) = req.params.num_ctx {
        options.insert("num_ctx".into(), serde_json::json!(c));
    }
    if let Some(m) = req.params.max_tokens {
        options.insert("num_predict".into(), serde_json::json!(m));
    }

    let mut messages = Vec::with_capacity(req.messages.len());
    for msg in &req.messages {
        let (content, images) = flatten_content(&msg.content);
        let mut m = serde_json::json!({
            "role": msg.role,
            "content": content,
        });
        if !images.is_empty() {
            m["images"] = serde_json::json!(images);
        }
        messages.push(m);
    }

    let mut body = serde_json::json!({
        "model": req.model,
        "messages": messages,
        "stream": true,
        "keep_alive": req.params.keep_alive.clone().unwrap_or_else(|| "5m".into()),
    });
    if !options.is_empty() {
        body["options"] = serde_json::Value::Object(options);
    }
    if let Some(schema) = &req.params.json_schema {
        body["format"] = schema.clone();
    }
    body
}

/// Text parts joined into `content`; image parts (M3 seam) become base64
/// entries in the Ollama `images[]` array.
fn flatten_content(content: &ChatContent) -> (String, Vec<String>) {
    match content {
        ChatContent::Text(text) => (text.clone(), Vec::new()),
        ChatContent::Parts(parts) => {
            let mut text = String::new();
            let mut images = Vec::new();
            for part in parts {
                match part {
                    crate::types::ContentPart::Text { text: t } => text.push_str(t),
                    crate::types::ContentPart::Image { data_base64, .. } => {
                        if let Some(b64) = data_base64 {
                            images.push(b64.clone());
                        }
                    }
                }
            }
            (text, images)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChatRole, ChatParams};

    #[test]
    fn parse_tags_full_fixture() {
        let fixture = serde_json::json!({
            "models": [
                {
                    "name": "gemma3:4b", "model": "gemma3:4b",
                    "modified_at": "2026-09-15T12:00:00Z", "size": 3338801804u64,
                    "digest": "a1b2", "capabilities": ["completion", "vision", "tools", "thinking"],
                    "context_length": 131072,
                    "details": {
                        "parent_model": "", "format": "gguf", "family": "gemma3",
                        "families": ["gemma3"], "parameter_size": "4.3B",
                        "quantization_level": "Q4_K_M"
                    }
                },
                { "name": "lfm2.5:latest", "model": "lfm2.5:latest", "size": 5_200_000_000u64 }
            ]
        });
        let models = parse_tags(&fixture).unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "gemma3:4b");
        assert_eq!(models[0].size_bytes, Some(3338801804));
        assert_eq!(models[0].capabilities, vec!["completion", "vision", "tools", "thinking"]);
        assert_eq!(models[0].context_length, Some(131072));
        assert_eq!(models[0].parameter_size.as_deref(), Some("4.3B"));
        assert_eq!(models[0].quantization_level.as_deref(), Some("Q4_K_M"));
        assert!(models[1].capabilities.is_empty());
        assert_eq!(models[1].context_length, None);
    }

    #[test]
    fn build_chat_body_defaults_and_options() {
        let req = ChatRequest {
            endpoint_id: "ep_local_ollama".into(),
            model: "gemma3:4b".into(),
            messages: vec![ChatMessage {
                id: "m1".into(),
                role: ChatRole::User,
                content: ChatContent::Text("hello".into()),
                tool_calls: None,
                tool_call_id: None,
            }],
            tools: None,
            params: ChatParams {
                temperature: Some(0.7),
                top_p: None,
                num_ctx: Some(8192),
                max_tokens: None,
                json_schema: None,
                keep_alive: Some("10m".into()),
            },
        };
        let body = build_chat_body(&req);
        assert_eq!(body["model"], "gemma3:4b");
        assert_eq!(body["stream"], true);
        assert_eq!(body["keep_alive"], "10m");
        let temperature = body["options"]["temperature"].as_f64().unwrap();
        assert!((temperature - 0.7).abs() < 1e-3, "temperature: {temperature}");
        assert_eq!(body["options"]["num_ctx"], 8192);
        assert!(body["options"].get("top_p").is_none());
        assert!(body.get("format").is_none());
        assert_eq!(body["messages"][0]["content"], "hello");
    }

    #[test]
    fn build_chat_body_schema_becomes_format() {
        let req = ChatRequest {
            endpoint_id: "e".into(),
            model: "m".into(),
            messages: vec![],
            tools: None,
            params: ChatParams {
                json_schema: Some(serde_json::json!({"type": "object"})),
                ..Default::default()
            },
        };
        let body = build_chat_body(&req);
        assert_eq!(body["format"], serde_json::json!({"type": "object"}));
    }

    /// Live check against the real Ollama (run explicitly:
    /// `cargo test -- --ignored`).
    #[tokio::test]
    #[ignore = "requires local Ollama running on :11434"]
    async fn live_list_models() {
        let http = reqwest::Client::new();
        let adapter = OllamaAdapter::new("ep_local_ollama", "http://127.0.0.1:11434", http);
        let models = adapter.list_models_impl().await.expect("live list_models");
        println!("live models: {models:#?}");
        assert!(!models.is_empty());
    }
}