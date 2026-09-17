//! Endpoint profile CRUD + connection test (design §5.1). The built-in local
//! Ollama (`ep_local_ollama`) is synthesized from the `ollama.base_url`
//! setting — it is always listed first, is not deletable, and its base URL is
//! edited through the ordinary settings key.

use serde::Deserialize;
use tauri::State;

use crate::{
    error::CmdError,
    ids,
    providers::{probe, secrets},
    state::AppState,
    types::{EndpointProfile, EndpointTestResult},
};

/// The always-present built-in profile (settings-driven, not a table row).
pub fn builtin_profile(base_url: &str) -> EndpointProfile {
    EndpointProfile {
        id: crate::state::DEFAULT_ENDPOINT.to_string(),
        kind: "ollama".into(),
        name: "Local Ollama".into(),
        base_url: base_url.to_string(),
        api_key_ref: None,
        headers: Default::default(),
        enabled: true,
        notes: Some("Built-in — the ollama.base_url setting".into()),
    }
}

#[tauri::command]
pub async fn list_endpoint_profiles(
    state: State<'_, AppState>,
) -> Result<Vec<EndpointProfile>, CmdError> {
    let mut profiles = state.db.list_endpoint_profiles().await?;
    let builtin = builtin_profile(&state.settings.get_or(
        crate::settings::keys::OLLAMA_BASE_URL,
        "http://127.0.0.1:11434",
    ));
    profiles.insert(0, builtin);
    Ok(profiles)
}

#[tauri::command]
pub async fn save_endpoint_profile(
    state: State<'_, AppState>,
    profile: EndpointProfile,
) -> Result<EndpointProfile, CmdError> {
    validate(&profile)?;
    let mut profile = profile;
    if profile.id.trim().is_empty() {
        profile.id = format!("ep_{}", ids::new_id());
    }
    if profile.id == crate::state::DEFAULT_ENDPOINT {
        return Err(CmdError::new(
            "builtin_profile",
            "The built-in Local Ollama profile is edited through its base URL setting",
        ));
    }
    let now = ids::now_ms();
    state.db.upsert_endpoint_profile(profile.clone(), now).await?;
    state.rebuild_providers().await;
    Ok(profile)
}

#[tauri::command]
pub async fn delete_endpoint_profile(
    state: State<'_, AppState>,
    id: String,
) -> Result<(), CmdError> {
    if id == crate::state::DEFAULT_ENDPOINT {
        return Err(CmdError::new(
            "builtin_profile",
            "The built-in Local Ollama profile cannot be deleted",
        ));
    }
    let existing = state.db.get_endpoint_profile(id.clone()).await?;
    if state.db.delete_endpoint_profile(id.clone()).await? {
        // Best effort: an orphaned keyring entry is harmless but untidy.
        if let Some(key_ref) = existing.and_then(|p| p.api_key_ref) {
            if let Err(e) = secrets::delete_api_key(&key_ref) {
                log::warn!("keyring cleanup failed for {id}: {e}");
            }
        }
        state.rebuild_providers().await;
    }
    Ok(())
}

/// Set (or overwrite) the endpoint's API key — stored in Windows Credential
/// Manager; the row keeps only the handle.
#[tauri::command]
pub async fn set_endpoint_api_key(
    state: State<'_, AppState>,
    id: String,
    secret: String,
) -> Result<(), CmdError> {
    let key_ref = secrets::ref_for(&id);
    secrets::store_api_key(&key_ref, &secret).map_err(CmdError::internal)?;
    let result = apply_key_ref(&state, id, Some(key_ref.clone())).await;
    if result.is_err() {
        // Don't leave a secret behind for a profile that no longer exists.
        let _ = secrets::delete_api_key(&key_ref);
    }
    result
}

#[tauri::command]
pub async fn clear_endpoint_api_key(
    state: State<'_, AppState>,
    id: String,
) -> Result<(), CmdError> {
    let key_ref = secrets::ref_for(&id);
    secrets::delete_api_key(&key_ref).map_err(CmdError::internal)?;
    apply_key_ref(&state, id, None).await
}

async fn apply_key_ref(
    state: &State<'_, AppState>,
    id: String,
    key_ref: Option<String>,
) -> Result<(), CmdError> {
    let mut profile = state
        .db
        .get_endpoint_profile(id.clone())
        .await?
        .ok_or_else(|| CmdError::internal(format!("unknown endpoint: {id}")))?;
    profile.api_key_ref = key_ref;
    state
        .db
        .upsert_endpoint_profile(profile, ids::now_ms())
        .await?;
    // A key change can flip a provider from 401 to working — refresh.
    state.rebuild_providers().await;
    Ok(())
}

/// Connection test (§5.1): GET /api/tags or GET /v1/models with latency +
/// model count. Unreachable servers return `ok: false` inline rather than an
/// IPC error so the dialog can show them next to the form.
#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TestEndpointArgs {
    /// "ollama" | "openai"
    pub kind: String,
    pub base_url: String,
    /// Profile id — when given, the stored keyring key rides along.
    #[serde(default)]
    pub endpoint_id: Option<String>,
}

#[tauri::command]
pub async fn test_endpoint(
    state: State<'_, AppState>,
    args: TestEndpointArgs,
) -> Result<EndpointTestResult, CmdError> {
    let mut api_key = None;
    if let Some(id) = &args.endpoint_id {
        let key_ref = secrets::ref_for(id);
        match secrets::read_api_key(&key_ref) {
            Ok(key) => api_key = key,
            Err(e) => log::warn!("keyring read for {id} failed: {e}"),
        }
    }

    let result = probe::probe_endpoint(&state.http, &args.kind, &args.base_url, api_key.as_deref())
        .await;
    match result {
        Ok(probe::ProbeResult { latency_ms, model_count, models }) => Ok(EndpointTestResult {
            ok: true,
            latency_ms,
            model_count,
            models,
            error: None,
        }),
        Err(e) => Ok(EndpointTestResult {
            ok: false,
            latency_ms: 0,
            model_count: 0,
            models: Vec::new(),
            error: Some(e.to_string()),
        }),
    }
}

/// §5.6 hand-off policy for an endpoint: "ask" (prompt on arrival, the
/// default), "always" (auto-apply in the UI), or "never" (suppress the
/// prompt). Persisted as the `handoff.<endpoint_id>` setting.
#[tauri::command]
pub async fn set_handoff_policy(
    state: State<'_, AppState>,
    endpoint_id: String,
    policy: String,
) -> Result<(), CmdError> {
    if !matches!(policy.as_str(), "ask" | "always" | "never") {
        return Err(CmdError::new(
            "invalid_policy",
            "policy must be ask, always, or never",
        ));
    }
    let key = crate::handoff::policy_key(endpoint_id.trim());
    state.db.set_setting(&key, &policy).await?;
    state.settings.set(&key, policy);
    Ok(())
}

fn validate(profile: &EndpointProfile) -> Result<(), CmdError> {
    if profile.name.trim().is_empty() {
        return Err(CmdError::new("invalid_profile", "Name is required"));
    }
    let base = profile.base_url.trim();
    if base.is_empty() {
        return Err(CmdError::new("invalid_profile", "Base URL is required"));
    }
    let parsed = url::Url::parse(base)
        .map_err(|e| CmdError::new("invalid_profile", format!("Bad base URL: {e}")))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(CmdError::new(
            "invalid_profile",
            "Base URL must start with http:// or https://",
        ));
    }
    if profile.kind != "ollama" && profile.kind != "openai" {
        return Err(CmdError::new(
            "invalid_profile",
            "kind must be \"ollama\" or \"openai\"",
        ));
    }
    Ok(())
}