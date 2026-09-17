//! Endpoint probing (design §5.1): a lightweight connection test that also
//! feeds discovery — `GET /api/tags` for Ollama, `GET /v1/models` for
//! OpenAI-compatible servers. Returns latency + model ids so the settings
//! dialog can surface both inline.

use super::ProviderError;

pub struct ProbeResult {
    pub latency_ms: u64,
    pub model_count: usize,
    /// First few model ids, for a quick eyeball check in the UI.
    pub models: Vec<String>,
}

/// Probe an endpoint. `api_key` (when present) is sent as a Bearer token.
pub async fn probe_endpoint(
    http: &reqwest::Client,
    kind: &str,
    base_url: &str,
    api_key: Option<&str>,
) -> Result<ProbeResult, ProviderError> {
    match kind {
        "openai" => {
            let url = openai_models_url(base_url);
            let mut req = http.get(&url);
            if let Some(key) = api_key {
                req = req.bearer_auth(key);
            }
            let started = std::time::Instant::now();
            let resp = req
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
                .map_err(|e| ProviderError::Malformed(format!("models list: {e}")))?;
            let models = parse_models_list(&value)
                .map_err(|e| ProviderError::Malformed(format!("models list: {e}")))?;
            Ok(ProbeResult {
                latency_ms: started.elapsed().as_millis() as u64,
                model_count: models.len(),
                models: models.into_iter().take(8).collect(),
            })
        }
        // Default (and "ollama"): the native Ollama tags endpoint.
        _ => {
            let url = format!("{}/api/tags", base_url.trim_end_matches('/'));
            let started = std::time::Instant::now();
            let resp = http
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
                .map_err(|e| ProviderError::Malformed(format!("api/tags: {e}")))?;
            let models = super::ollama::parse_tags(&value)
                .map_err(|e| ProviderError::Malformed(format!("api/tags: {e}")))?;
            Ok(ProbeResult {
                latency_ms: started.elapsed().as_millis() as u64,
                model_count: models.len(),
                models: models.into_iter().map(|m| m.id).take(8).collect(),
            })
        }
    }
}

/// `GET {base}/v1/models` URL: the base may be given with or without a
/// trailing slash, and with or without an explicit `/v1` path (LM Studio
/// presets are documented as `:1234/v1`, llama.cpp as `:8080`). Append `/v1`
/// only when the caller didn't already include it.
pub fn openai_models_url(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.rsplit('/').next() == Some("v1") {
        format!("{base}/models")
    } else {
        format!("{base}/v1/models")
    }
}

/// Parse an OpenAI-compatible `/v1/models` body: `{"data": [{"id": …}, …]}`
/// (some servers also include `object: "list"`; both spellings of the array
/// key are tolerated: `data` is standard, `models` appears in the wild).
pub fn parse_models_list(value: &serde_json::Value) -> Result<Vec<String>, String> {
    let arr = value
        .get("data")
        .and_then(|v| v.as_array())
        .or_else(|| value.get("models").and_then(|v| v.as_array()))
        .ok_or_else(|| "expected a `data` array of model objects".to_string())?;
    Ok(arr
        .iter()
        .filter_map(|m| {
            m.get("id")
                .and_then(|v| v.as_str())
                .map(String::from)
                .or_else(|| m.get("name").and_then(|v| v.as_str()).map(String::from))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_url_normalizes_base_variants() {
        assert_eq!(
            openai_models_url("http://127.0.0.1:1234"),
            "http://127.0.0.1:1234/v1/models"
        );
        assert_eq!(
            openai_models_url("http://127.0.0.1:1234/"),
            "http://127.0.0.1:1234/v1/models"
        );
        // Presets documented with an explicit /v1 keep it exactly once.
        assert_eq!(
            openai_models_url("http://127.0.0.1:1234/v1"),
            "http://127.0.0.1:1234/v1/models"
        );
        assert_eq!(
            openai_models_url("http://127.0.0.1:1234/v1/"),
            "http://127.0.0.1:1234/v1/models"
        );
        // A deeper custom base path is preserved.
        assert_eq!(
            openai_models_url("http://gw.corp/api/v1"),
            "http://gw.corp/api/v1/models"
        );
    }

    #[test]
    fn parse_models_list_standard_fixture() {
        let v = serde_json::json!({
            "object": "list",
            "data": [
                {"id": "qwen2.5-7b-instruct", "object": "model", "owned_by": "lmstudio"},
                {"id": "gpt-4o-mini", "object": "model"}
            ]
        });
        assert_eq!(
            parse_models_list(&v).unwrap(),
            vec!["qwen2.5-7b-instruct", "gpt-4o-mini"]
        );
    }

    #[test]
    fn parse_models_list_name_fallback_and_errors() {
        // Some servers put the name under `name` instead of `id`.
        let v = serde_json::json!({"data": [{"name": "local-model"}]});
        assert_eq!(parse_models_list(&v).unwrap(), vec!["local-model"]);

        // Malformed bodies are errors, not silent empties.
        assert!(parse_models_list(&serde_json::json!({"models_count": 2})).is_err());
        assert!(parse_models_list(&serde_json::json!({"data": "nope"})).is_err());
    }
}