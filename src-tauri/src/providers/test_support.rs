//! Test-only provider used by chat orchestration and router tests: replays a
//! scripted event list, optionally parking on the cancellation token so tests
//! can drive Stop paths.

#![cfg(test)]

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use super::{EndpointKind, Provider, ProviderError};
use crate::{
    state::AppState,
    types::{ChatRequest, ModelInfo, StreamEvent},
};

pub struct FakeProvider {
    /// Per-call scripts: the i-th `chat` call replays entry i; the last entry
    /// repeats for any extra calls. A single-entry vec behaves like `scripted`.
    scripts: Arc<Mutex<VecDeque<Vec<StreamEvent>>>>,
    /// Index of the event after which the provider parks on `cancelled()`.
    wait_cancel_at: Option<usize>,
    /// Park before sending anything at all (timeout tests).
    park: bool,
    /// Requests received by `chat`, for asserting what was sent upstream.
    pub received: Arc<Mutex<Vec<ChatRequest>>>,
}

impl FakeProvider {
    pub fn scripted(events: Vec<StreamEvent>) -> Self {
        Self::scripted_sequence(vec![events])
    }

    /// Each `chat` call replays the next script; the last one repeats.
    pub fn scripted_sequence(scripts: Vec<Vec<StreamEvent>>) -> Self {
        Self {
            scripts: Arc::new(Mutex::new(scripts.into())),
            wait_cancel_at: None,
            park: false,
            received: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Parks after the first event — tests flip the token to exercise Stop.
    pub fn cancelling_after(events: Vec<StreamEvent>) -> Self {
        let mut p = Self::scripted(events);
        p.wait_cancel_at = Some(1);
        p
    }

    /// Never sends anything and never completes — for timeout tests.
    pub fn parking() -> Self {
        Self {
            scripts: Arc::new(Mutex::new(VecDeque::new())),
            wait_cancel_at: None,
            park: true,
            received: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// All requests seen so far (classify calls included).
    pub fn requests(&self) -> Vec<ChatRequest> {
        self.received.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

impl Provider for FakeProvider {
    fn id(&self) -> &str {
        "ep_local_ollama"
    }

    fn kind(&self) -> EndpointKind {
        EndpointKind::Ollama
    }

    fn list_models(&self) -> futures::future::BoxFuture<'_, Result<Vec<ModelInfo>, ProviderError>> {
        Box::pin(async { Ok(vec![]) })
    }

    fn chat(
        &self,
        req: ChatRequest,
        tx: mpsc::Sender<StreamEvent>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> futures::future::BoxFuture<'_, ()> {
        let events = {
            let mut scripts = self.scripts.lock().unwrap_or_else(|p| p.into_inner());
            if scripts.len() > 1 {
                scripts.pop_front().unwrap_or_default()
            } else {
                scripts.front().cloned().unwrap_or_default()
            }
        };
        self.received
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(req);
        let wait_cancel_at = self.wait_cancel_at;
        let park = self.park;
        Box::pin(async move {
            if park {
                cancel.cancelled().await;
                return;
            }
            for (i, ev) in events.into_iter().enumerate() {
                if wait_cancel_at == Some(i) {
                    cancel.cancelled().await;
                    return;
                }
                if tx.send(ev).await.is_err() {
                    return;
                }
            }
        })
    }
}

/// An AppState over one FakeProvider + a temp DB, with default chat settings.
/// Returns the TempDir to keep it alive for the duration of the test.
pub fn app_with(provider: FakeProvider) -> (tempfile::TempDir, AppState) {
    app_with_settings(provider, &[])
}

/// Same, with extra settings merged in (triad.* tests).
pub fn app_with_settings(
    provider: FakeProvider,
    extra: &[(&str, &str)],
) -> (tempfile::TempDir, AppState) {
    let dir = tempfile::tempdir().unwrap();
    let db = crate::db::Database::open(&dir.path().join("t.db")).unwrap();
    let providers: HashMap<String, std::sync::Arc<dyn Provider>> = HashMap::from([(
        "ep_local_ollama".to_string(),
        std::sync::Arc::new(provider) as std::sync::Arc<dyn Provider>,
    )]);
    let mut pairs = HashMap::from([
        ("ollama.base_url".to_string(), "http://127.0.0.1:11434".to_string()),
        ("chat.temperature".to_string(), "0.7".to_string()),
        ("chat.context_tokens".to_string(), "8192".to_string()),
        ("chat.keep_alive".to_string(), "10m".to_string()),
    ]);
    for (k, v) in extra {
        pairs.insert(k.to_string(), v.to_string());
    }
    let settings = crate::settings::SettingsCache::new(pairs);
    let state = AppState::new(db, settings, reqwest::Client::new(), providers);
    (dir, state)
}