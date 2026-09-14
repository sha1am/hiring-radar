use crate::config::Config;
use crate::draft::Drafter;
use crate::settings::Settings;
use sqlx::SqlitePool;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub cfg: Arc<Config>,
    pub http: reqwest::Client,
    pub drafter: Arc<dyn Drafter>,
    /// Fires a tick to connected dashboards (via SSE) whenever the queue changes.
    pub events: broadcast::Sender<()>,
    /// Live, user-editable configuration. Held behind an RwLock around an Arc so
    /// readers clone a cheap snapshot and never hold the lock across an await —
    /// crawl loops and the release engine read this on every pass.
    pub settings: Arc<RwLock<Arc<Settings>>>,
}

impl AppState {
    /// Point-in-time view of the settings. Take one at the top of an operation
    /// and use it throughout, so a save mid-pass can't change the rules
    /// halfway through a release decision.
    pub async fn settings(&self) -> Arc<Settings> {
        self.settings.read().await.clone()
    }

    pub async fn set_settings(&self, s: Settings) {
        *self.settings.write().await = Arc::new(s);
    }

    pub fn notify_ui(&self) {
        let _ = self.events.send(());
    }
}
