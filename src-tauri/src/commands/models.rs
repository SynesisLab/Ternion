//! Model discovery across all enabled endpoint profiles (design §5.1/§5.4).
//! The built-in local Ollama is probed first and keeps bare ids (every pre-M3
//! pin/role reference stays valid); profile models get `"model@endpoint"`
//! references. Discovery facts are layered with user capability records from
//! the `models` table — a record wins per field and survives discovery gaps.
//! A down endpoint degrades to a warning — the picker still shows every
//! endpoint that answered.

use serde::Deserialize;
use tauri::State;

use crate::{
    error::CmdError,
    ids,
    providers::{ollama, parse_model_ref, qualified_model_ref, Provider},
    state::{AppState, DEFAULT_ENDPOINT},
    types::{EndpointProfile, ModelInfo, ModelRecord},
};

/// Merge user capability records over discovery facts (§5.4): a record wins
/// per field when it carries a value. Records for models the discovery sweep
/// never saw are appended so user tags survive an offline endpoint.
fn merge_records(discovered: Vec<ModelInfo>, records: Vec<ModelRecord>) -> Vec<ModelInfo> {
    let mut all: Vec<ModelInfo> = discovered
        .into_iter()
        .map(|mut m| {
            let bare = parse_model_ref(&m.id).1;
            if let Some(rec) = records
                .iter()
                .find(|r| r.endpoint_id == m.endpoint_id && r.model == bare)
            {
                if !rec.capabilities.is_empty() {
                    m.capabilities = rec.capabilities.clone();
                }
                if let Some(ctx) = rec.context_tokens {
                    m.context_length = Some(ctx);
                }
            }
            m
        })
        .collect();

    for rec in &records {
        let already = all.iter().any(|m| {
            m.endpoint_id == rec.endpoint_id && parse_model_ref(&m.id).1 == rec.model
        });
        if !already {
            let id = if rec.endpoint_id == DEFAULT_ENDPOINT {
                rec.model.clone()
            } else {
                qualified_model_ref(&rec.endpoint_id, &rec.model)
            };
            all.push(ModelInfo {
                id,
                display_name: rec.model.clone(),
                endpoint_id: rec.endpoint_id.clone(),
                size_bytes: None,
                parameter_size: None,
                quantization_level: None,
                family: None,
                context_length: rec.context_tokens,
                capabilities: rec.capabilities.clone(),
            });
        }
    }
    all
}

#[tauri::command]
pub async fn list_models(state: State<'_, AppState>) -> Result<Vec<ModelInfo>, CmdError> {
    let mut all: Vec<ModelInfo> = Vec::new();

    if let Ok(provider) = state.provider_for(DEFAULT_ENDPOINT) {
        match provider.list_models().await {
            Ok(models) => all.extend(models.into_iter().map(|mut m| {
                m.endpoint_id = DEFAULT_ENDPOINT.to_string();
                m
            })),
            Err(e) => log::warn!("builtin model discovery failed: {e}"),
        }
    }

    let profiles: Vec<EndpointProfile> = state
        .db
        .list_endpoint_profiles()
        .await?
        .into_iter()
        .filter(|p| p.enabled)
        .collect();
    for profile in &profiles {
        let provider: std::sync::Arc<dyn Provider> = match state.provider_for(&profile.id) {
            Ok(p) => p,
            Err(_) => continue,
        };
        match provider.list_models().await {
            Ok(models) => all.extend(models.into_iter().map(|mut m| {
                m.id = qualified_model_ref(&profile.id, &m.id);
                m.endpoint_id = profile.id.clone();
                m
            })),
            Err(e) => log::warn!(
                "model discovery for '{}' ({}) failed: {e}",
                profile.name,
                profile.id
            ),
        }
    }

    let mut merged = merge_records(all, visible_records(&state, &profiles).await?);

    // §6.5d: enabled Pipe manifests surface as pseudo-models behind the
    // synthetic pipe endpoint — selectable, pinned, and served by the
    // PipeProvider like any other model.
    for row in state.db.list_owui_tools().await.unwrap_or_default() {
        if row.enabled && row.kind == "pipe" {
            merged.push(ModelInfo {
                id: qualified_model_ref(crate::owui::PIPE_ENDPOINT, &crate::owui::pipe_ref(&row.name)),
                display_name: row.name.clone(),
                endpoint_id: crate::owui::PIPE_ENDPOINT.to_string(),
                size_bytes: None,
                parameter_size: None,
                quantization_level: None,
                family: Some("openwebui-pipe".to_string()),
                context_length: None,
                capabilities: Vec::new(),
            });
        }
    }

    // Cache capability facts for the policy engine's hard rules (§5.4).
    state.update_model_registry(merged.clone());
    Ok(merged)
}

/// Records only count for the built-in endpoint and *enabled* profiles — a
/// disabled endpoint's tags must not surface in the picker.
async fn visible_records(
    state: &State<'_, AppState>,
    profiles: &[EndpointProfile],
) -> Result<Vec<ModelRecord>, CmdError> {
    let records = state.db.list_model_records().await?;
    Ok(records
        .into_iter()
        .filter(|r| {
            r.endpoint_id == DEFAULT_ENDPOINT
                || profiles.iter().any(|p| p.id == r.endpoint_id)
        })
        .collect())
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SaveModelRecordArgs {
    pub endpoint_id: String,
    /// Bare model name (no @endpoint suffix).
    pub model: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub context_tokens: Option<u32>,
}

#[tauri::command]
pub async fn save_model_record(
    state: State<'_, AppState>,
    args: SaveModelRecordArgs,
) -> Result<ModelRecord, CmdError> {
    if args.model.trim().is_empty() {
        return Err(CmdError::new("invalid_record", "Model name is required"));
    }
    // Dedupe + trim; tags are lowercase in the registry's vocabulary.
    let mut capabilities: Vec<String> = args
        .capabilities
        .iter()
        .map(|c| c.trim().to_lowercase())
        .filter(|c| !c.is_empty())
        .collect();
    capabilities.sort();
    capabilities.dedup();

    let record = ModelRecord {
        endpoint_id: args.endpoint_id,
        model: args.model.trim().to_string(),
        capabilities,
        context_tokens: args.context_tokens,
        role: None,
        vram_estimate_gb: None,
        // A manual save is a confirmation too (§5.4 `verifiedAt`).
        verified_at: Some(ids::now_ms()),
    };
    state.db.upsert_model_record(record.clone()).await?;
    refresh_registry(&state).await?;
    Ok(record)
}

#[tauri::command]
pub async fn clear_model_record(
    state: State<'_, AppState>,
    endpoint_id: String,
    model: String,
) -> Result<(), CmdError> {
    state.db.delete_model_record(endpoint_id, model).await?;
    refresh_registry(&state).await?;
    Ok(())
}

/// Re-probe one Ollama model's facts via `/api/show` and store them as the
/// record (§5.4). OpenAI-compatible endpoints have no equivalent — the
/// command rejects them so the UI can hide the action.
#[tauri::command]
pub async fn verify_model(
    state: State<'_, AppState>,
    endpoint_id: String,
    model: String,
) -> Result<ModelRecord, CmdError> {
    let base_url = if endpoint_id == DEFAULT_ENDPOINT {
        state
            .settings
            .get_or(crate::settings::keys::OLLAMA_BASE_URL, "http://127.0.0.1:11434")
    } else {
        state
            .db
            .get_endpoint_profile(endpoint_id.clone())
            .await?
            .filter(|p| p.kind == "ollama")
            .map(|p| p.base_url)
            .ok_or_else(|| {
                CmdError::new(
                    "invalid_record",
                    "Verify is available for Ollama endpoints only",
                )
            })?
    };

    let show = ollama::show_model(&state.http, &base_url, &model)
        .await
        .map_err(CmdError::internal)?;

    let existing = state
        .db
        .list_model_records()
        .await?
        .into_iter()
        .find(|r| r.endpoint_id == endpoint_id && r.model == model);
    // Keep user-tuned fields the show response can't express (§5.4 extras).
    let (role, vram) = existing
        .map(|e| (e.role, e.vram_estimate_gb))
        .unwrap_or((None, None));

    let record = ModelRecord {
        endpoint_id,
        model,
        capabilities: show.capabilities,
        context_tokens: show.context_length,
        role,
        vram_estimate_gb: vram,
        verified_at: Some(ids::now_ms()),
    };
    state.db.upsert_model_record(record.clone()).await?;
    refresh_registry(&state).await?;
    Ok(record)
}

/// Re-run discovery + merge so in-memory capability facts (policy engine,
/// picker) reflect the stored records without waiting for the next sweep.
async fn refresh_registry(state: &State<'_, AppState>) -> Result<Vec<ModelInfo>, CmdError> {
    let mut all: Vec<ModelInfo> = Vec::new();
    if let Ok(provider) = state.provider_for(DEFAULT_ENDPOINT) {
        if let Ok(models) = provider.list_models().await {
            all.extend(models.into_iter().map(|mut m| {
                m.endpoint_id = DEFAULT_ENDPOINT.to_string();
                m
            }));
        }
    }
    let profiles: Vec<EndpointProfile> = state
        .db
        .list_endpoint_profiles()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.enabled)
        .collect();
    for profile in &profiles {
        if let Ok(provider) = state.provider_for(&profile.id) {
            if let Ok(models) = provider.list_models().await {
                all.extend(models.into_iter().map(|mut m| {
                    m.id = qualified_model_ref(&profile.id, &m.id);
                    m.endpoint_id = profile.id.clone();
                    m
                }));
            }
        }
    }
    let merged = merge_records(
        all,
        visible_records(state, &profiles).await.unwrap_or_default(),
    );
    state.update_model_registry(merged.clone());
    Ok(merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn discovered(id: &str, endpoint_id: &str, caps: &[&str]) -> ModelInfo {
        ModelInfo {
            id: id.into(),
            display_name: id.into(),
            endpoint_id: endpoint_id.into(),
            size_bytes: None,
            parameter_size: None,
            quantization_level: None,
            family: None,
            context_length: None,
            capabilities: caps.iter().map(|c| c.to_string()).collect(),
        }
    }

    #[test]
    fn records_override_discovered_fields() {
        let d = vec![
            discovered("gemma3:4b", DEFAULT_ENDPOINT, &["completion"]),
            discovered("qwen2.5vl:7b@ep_remote", "ep_remote", &[]),
        ];
        let records = vec![
            ModelRecord {
                endpoint_id: "ep_remote".into(),
                model: "qwen2.5vl:7b".into(),
                capabilities: vec!["vision".into(), "tools".into()],
                context_tokens: Some(32768),
                role: None,
                vram_estimate_gb: None,
                verified_at: Some(1),
            },
            // Empty record fields leave discovery facts alone.
            ModelRecord {
                endpoint_id: DEFAULT_ENDPOINT.into(),
                model: "gemma3:4b".into(),
                capabilities: Vec::new(),
                context_tokens: None,
                role: None,
                vram_estimate_gb: None,
                verified_at: Some(2),
            },
        ];
        let merged = merge_records(d, records);
        assert_eq!(merged.len(), 2);
        assert_eq!(
            merged[0].capabilities,
            vec!["completion"],
            "empty record doesn't clobber discovery"
        );
        assert_eq!(
            merged[1].capabilities,
            vec!["vision", "tools"],
            "user tags win over an empty discovery list"
        );
        assert_eq!(merged[1].context_length, Some(32768));
        assert_eq!(merged[1].id, "qwen2.5vl:7b@ep_remote");
    }

    #[test]
    fn records_survive_discovery_gaps() {
        // The endpoint is down; the user's tags must still reach the picker
        // and the policy engine.
        let records = vec![ModelRecord {
            endpoint_id: "ep_remote".into(),
            model: "gpt-4o".into(),
            capabilities: vec!["vision".into()],
            context_tokens: Some(128000),
            role: None,
            vram_estimate_gb: None,
            verified_at: Some(1),
        }];
        let merged = merge_records(Vec::new(), records);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].id, "gpt-4o@ep_remote");
        assert_eq!(merged[0].endpoint_id, "ep_remote");
        assert_eq!(merged[0].capabilities, vec!["vision"]);
        assert_eq!(merged[0].context_length, Some(128000));

        // Builtin records keep bare ids.
        let merged = merge_records(
            Vec::new(),
            vec![ModelRecord {
                endpoint_id: DEFAULT_ENDPOINT.into(),
                model: "gemma3:4b".into(),
                capabilities: vec!["thinking".into()],
                context_tokens: None,
                role: None,
                vram_estimate_gb: None,
                verified_at: Some(1),
            }],
        );
        assert_eq!(merged[0].id, "gemma3:4b");
        assert_eq!(merged[0].endpoint_id, DEFAULT_ENDPOINT);
    }

    #[test]
    fn record_not_duplicated_when_discovery_and_record_match() {
        let d = vec![discovered("qwen2.5vl:7b@ep_remote", "ep_remote", &[])];
        let records = vec![ModelRecord {
            endpoint_id: "ep_remote".into(),
            model: "qwen2.5vl:7b".into(),
            capabilities: vec!["vision".into()],
            context_tokens: None,
            role: None,
            vram_estimate_gb: None,
            verified_at: Some(1),
        }];
        let merged = merge_records(d, records);
        assert_eq!(merged.len(), 1, "record merges into the discovered row");
        assert_eq!(merged[0].capabilities, vec!["vision"]);
    }
}