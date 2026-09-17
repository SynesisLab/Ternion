//! Provider hand-off (design §5.6): endpoints are probed on a periodic
//! timer, and when one transitions from down to available the UI is offered
//! the switch with one prompt — "…detected — switch roles to it?" — never
//! silently. The switch itself is the conversation's pinned model moving to
//! a qualified ref on the new endpoint, so the next turn routes there and
//! the §3.6 hand-off machinery (digest + recent turns) carries the thread.
//!
//! Policy hooks (§5.6): local-only mode (§3.10) suppresses offers for
//! non-local endpoints entirely; a per-endpoint policy setting
//! (`handoff.<endpoint_id>` = "ask" | "always" | "never") gates the prompt —
//! "never" suppresses it, "always" lets the UI auto-apply it. Cost-guarded
//! (non-local) endpoints are offered, never silently switched.

use std::collections::HashMap;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::settings::keys;
use crate::state::{AppState, DEFAULT_ENDPOINT};

/// Emitted when an endpoint becomes newly available.
pub const EVENT: &str = "ternion://handoff-available";

/// §5.6 "on a periodic timer" — slow by design: presence is a convenience,
/// not a heartbeat.
const POLL_MS: u64 = 60_000;
/// A presence probe must never hang the loop.
const PROBE_TIMEOUT_MS: u64 = 5_000;

pub const POLICY_ASK: &str = "ask";
pub const POLICY_ALWAYS: &str = "always";
pub const POLICY_NEVER: &str = "never";

/// The persisted per-endpoint policy setting key.
pub fn policy_key(endpoint_id: &str) -> String {
    format!("handoff.{endpoint_id}")
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffOffer {
    pub endpoint_id: String,
    pub name: String,
    /// "ollama" | "openai" — the UI shows a cost note for non-local kinds
    /// (§3.8 guardrail: cloud is offered, never silently switched).
    pub kind: String,
    pub local: bool,
    pub latency_ms: u64,
    pub model_count: usize,
    /// First model ids from the probe — the switch targets the first.
    pub models: Vec<String>,
    /// Persisted policy: "ask" | "always" (auto-apply in the UI) | "never".
    pub policy: String,
}

/// Pure transition rule: the first pass only seeds state — a provider that
/// is already up at launch is not "newly available"; offers fire on a
/// known-down → available transition (§5.6 "when a new provider becomes
/// available while the user is on another", including "when the original
/// returns").
fn is_newly_available(prev: Option<bool>, now: bool) -> bool {
    matches!(prev, Some(false)) && now
}

/// Whether a down→up transition on this endpoint should surface the switch
/// prompt: never-suppressed, local-only mode (§3.10) suppresses non-local,
/// and an offer without models cannot name a switch target.
fn should_offer(
    policy: &str,
    local_only: bool,
    local: bool,
    model_count: usize,
) -> bool {
    policy != POLICY_NEVER && (!local_only || local) && model_count > 0
}

/// One probe target: the built-in local Ollama plus every enabled profile.
struct Target {
    endpoint_id: String,
    name: String,
    kind: String,
    base_url: String,
    api_key: Option<String>,
}

impl Target {
    fn local(&self) -> bool {
        self.endpoint_id == DEFAULT_ENDPOINT || self.kind == "ollama"
    }
}

async fn targets(state: &AppState) -> Vec<Target> {
    let mut out = Vec::new();
    let base = state
        .settings
        .get_or(keys::OLLAMA_BASE_URL, "http://127.0.0.1:11434");
    out.push(Target {
        endpoint_id: DEFAULT_ENDPOINT.to_string(),
        name: "Ollama (local)".to_string(),
        kind: "ollama".to_string(),
        base_url: base,
        api_key: None,
    });
    let profiles = state.db.list_endpoint_profiles().await.unwrap_or_default();
    for p in profiles.iter().filter(|p| p.enabled) {
        let api_key = p
            .api_key_ref
            .as_deref()
            .and_then(|r| crate::providers::secrets::read_api_key(r).ok())
            .flatten();
        out.push(Target {
            endpoint_id: p.id.clone(),
            name: p.name.clone(),
            kind: p.kind.clone(),
            base_url: p.base_url.clone(),
            api_key,
        });
    }
    out
}

/// One presence pass: probe everything concurrently, diff against the
/// previous state, and emit offers for down→up transitions that pass the
/// policy gates. The first pass only seeds.
async fn run_pass(
    state: &AppState,
    emit: &(dyn Fn(HandoffOffer) + Send + Sync),
    prev: &mut HashMap<String, bool>,
) {
    let targets = targets(state).await;
    let local_only = crate::router::config::TriadConfig::load(&state.settings).local_only;
    let http = state.http.clone();

    let checks = targets
        .into_iter()
        .map(|t| {
            let http = http.clone();
            async move {
                let probe = tokio::time::timeout(
                    std::time::Duration::from_millis(PROBE_TIMEOUT_MS),
                    crate::providers::probe::probe_endpoint(&http, &t.kind, &t.base_url, t.api_key.as_deref()),
                )
                .await;
                let (available, latency_ms, models) = match probe {
                    Ok(Ok(r)) => (r.model_count > 0, r.latency_ms, r.models),
                    _ => (false, 0, Vec::new()),
                };
                (t, available, latency_ms, models)
            }
        })
        .collect::<Vec<_>>();
    let results = futures::future::join_all(checks).await;

    for (target, available, latency_ms, models) in results {
        if is_newly_available(prev.get(&target.endpoint_id).copied(), available) {
            let policy = state
                .settings
                .get_or(&policy_key(&target.endpoint_id), POLICY_ASK);
            if should_offer(&policy, local_only, target.local(), models.len()) {
                emit(HandoffOffer {
                    endpoint_id: target.endpoint_id.clone(),
                    name: target.name.clone(),
                    kind: target.kind.clone(),
                    local: target.local(),
                    latency_ms,
                    model_count: models.len(),
                    models: models.clone(),
                    policy: policy.clone(),
                });
            }
        }
        prev.insert(target.endpoint_id, available);
    }
}

/// The §5.6 watcher: probe on a timer, emit `ternion://handoff-available`
/// on down→up transitions. Spawned once at setup.
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut prev: HashMap<String, bool> = HashMap::new();
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(POLL_MS)).await;
            let state = app.state::<AppState>();
            run_pass(&state, &|offer| {
                let _ = app.emit(EVENT, offer);
            }, &mut prev)
            .await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newly_available_requires_a_known_down_state() {
        // First pass seeds — no offer.
        assert!(!is_newly_available(None, true));
        // The §5.6 transition: known-down → up.
        assert!(is_newly_available(Some(false), true));
        // Stays up / stays down — no repeated offers.
        assert!(!is_newly_available(Some(true), true));
        assert!(!is_newly_available(Some(false), false));
    }

    #[test]
    fn offer_gates_follow_policy_and_privacy() {
        assert!(should_offer(POLICY_ASK, false, false, 3));
        assert!(should_offer(POLICY_ALWAYS, false, false, 1));
        assert!(
            !should_offer(POLICY_NEVER, false, false, 3),
            "never suppresses the prompt (§5.6)"
        );
        // §3.10: local-only suppresses non-local providers entirely.
        assert!(!should_offer(POLICY_ASK, true, false, 3));
        assert!(should_offer(POLICY_ASK, true, true, 3));
        // An offer without models cannot name a switch target.
        assert!(!should_offer(POLICY_ASK, false, false, 0));
    }

    #[test]
    fn policy_key_namespaces_per_endpoint() {
        assert_eq!(policy_key("ep_x"), "handoff.ep_x");
    }

    #[test]
    fn target_locality_follows_kind_and_builtin() {
        let builtin = Target {
            endpoint_id: DEFAULT_ENDPOINT.into(),
            name: "x".into(),
            kind: "openai".into(),
            base_url: String::new(),
            api_key: None,
        };
        assert!(builtin.local(), "the built-in endpoint is always local");
        let remote_ollama = Target {
            endpoint_id: "ep_home".into(),
            name: "homelab".into(),
            kind: "ollama".into(),
            base_url: String::new(),
            api_key: None,
        };
        assert!(remote_ollama.local(), "self-hosted Ollama = no cost");
        let cloud = Target {
            endpoint_id: "ep_cloud".into(),
            name: "gateway".into(),
            kind: "openai".into(),
            base_url: String::new(),
            api_key: None,
        };
        assert!(!cloud.local());
    }
}