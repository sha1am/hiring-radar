use crate::config::Config;
use crate::draft::Drafter;
use sqlx::SqlitePool;
use std::sync::Arc;
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub cfg: Arc<Config>,
    pub http: reqwest::Client,
    pub drafter: Arc<dyn Drafter>,
    /// Fires a tick to connected dashboards (via SSE) whenever the queue changes.
    pub events: broadcast::Sender<()>,
}

impl AppState {
    pub fn notify_ui(&self) {
        let _ = self.events.send(());
    }
}
