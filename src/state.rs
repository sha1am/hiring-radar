use crate::config::Config;
use crate::draft::Drafter;
use crate::resume::Matcher;
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
    /// IDF corpus + the vectorised resume. Rebuilt when the resume changes;
    /// the corpus is updated as posts stream in.
    pub matcher: Arc<RwLock<Arc<Matcher>>>,
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

    pub async fn matcher(&self) -> Arc<Matcher> {
        self.matcher.read().await.clone()
    }

    /// Fold one post's terms into the IDF corpus. Kept short: clone-on-write of
    /// the whole matcher would be far too expensive per post, so this mutates
    /// in place under a brief write lock.
    pub async fn observe_corpus(&self, terms: &[String]) {
        let mut guard = self.matcher.write().await;
        let m = Arc::make_mut(&mut guard);
        m.corpus.observe(terms);
    }

    /// Re-vectorise the resume against the current corpus.
    pub async fn rebuild_resume(&self, resume_text: &str) {
        let mut guard = self.matcher.write().await;
        let m = Arc::make_mut(&mut guard);
        m.rebuild_resume(resume_text);
    }

    pub fn notify_ui(&self) {
        let _ = self.events.send(());
    }
}
