//! Test-only provider used by chat orchestration and router tests: replays a
//! scripted event list, optionally parking on the cancellation token so tests
//! can drive Stop paths.

#![cfg(test)]

use std::collections::HashMap;

use tokio::sync::mpsc;

use super::{EndpointKind, Provider, ProviderError};
use crate::{
    state::AppState,
    types::{ChatRequest, ModelInfo, StreamEvent},
};

pub struct FakeProvider {
    pub events: Vec<StreamEvent>,
    /// Index of the event after which the provider parks on `cancelled()`.
    pub wait_cancel_at: Option<usize>,
    /// Park before sending anything at all (timeout tests).
    pub park: bool,
}

impl FakeProvider {
    pub fn scripted(events: Vec<StreamEvent>) -> Self {
        Self {
            events,
            wait_cancel_at: None,
            park: false,
        }
    }

    /// Parks after the first event — tests flip the token to exercise Stop.
    pub fn cancelling_after(events: Vec<StreamEvent>) -> Self {
        Self {
            events,
            wait_cancel_at: Some(1),
            park: false,
        }
    }

    /// Never sends anything and never completes — for timeout tests.
    pub fn parking() -> Self {
        Self {
            events: vec![],
            wait_cancel_at: None,
            park: true,
        }
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
        _req: ChatRequest,
        tx: mpsc::Sender<StreamEvent>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> futures::future::BoxFuture<'_, ()> {
        let events = self.events.clone();
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

/// An AppState over one FakeProvider + a temp DB. Returns the TempDir to keep
/// it alive for the duration of the test.
pub fn app_with(provider: FakeProvider) -> (tempfile::TempDir, AppState) {
    let dir = tempfile::tempdir().unwrap();
    let db = crate::db::Database::open(&dir.path().join("t.db")).unwrap();
    let providers: HashMap<String, std::sync::Arc<dyn Provider>> = HashMap::from([(
        "ep_local_ollama".to_string(),
        std::sync::Arc::new(provider) as std::sync::Arc<dyn Provider>,
    )]);
    let settings = crate::settings::SettingsCache::new(HashMap::from([
        ("ollama.base_url".to_string(), "http://127.0.0.1:11434".to_string()),
        ("chat.temperature".to_string(), "0.7".to_string()),
        ("chat.context_tokens".to_string(), "8192".to_string()),
        ("chat.keep_alive".to_string(), "10m".to_string()),
    ]));
    let state = AppState::new(db, settings, reqwest::Client::new(), providers);
    (dir, state)
}