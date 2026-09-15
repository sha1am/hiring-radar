use crate::config::Config;
use crate::draft::Drafter;
use crate::resume::Matcher;
use crate::settings::Settings;
use crate::status::Status;
use sqlx::SqlitePool;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub cfg: Arc<Config>,
    pub http: reqwest::Client,
    pub drafter: Arc<dyn Drafter>,
    /// Reads structured facts out of a posting. A no-op unless a model is
    /// configured — the heuristics at ingest are what make the filters work.
    pub enricher: Arc<dyn crate::enrich::Enricher>,
    /// Fires a tick to connected dashboards (via SSE) whenever the queue changes.
    pub events: broadcast::Sender<()>,
    /// Live, user-editable configuration. Held behind an RwLock around an Arc so
    /// readers clone a cheap snapshot and never hold the lock across an await —
    /// crawl loops and the release engine read this on every pass.
    pub settings: Arc<RwLock<Arc<Settings>>>,
    /// IDF corpus + the vectorised resume. Rebuilt when the resume changes;
    /// the corpus is updated as posts stream in.
    pub matcher: Arc<RwLock<Arc<Matcher>>>,
    /// What each source is doing, so an empty board can explain itself.
    pub status: Arc<RwLock<Status>>,
    /// The candidate, as structured facts. Derived from the resume once at
    /// upload rather than per posting — it does not change between postings.
    pub profile: Arc<RwLock<Arc<crate::ats::Profile>>>,
}

impl AppState {
    /// Point-in-time view of the settings. Take one at the top of an operation
    /// and use it throughout, so a save mid-pass can't change the rules
    /// halfway through a release decision.
    pub async fn settings(&self) -> Arc<Settings> {
        self.settings.read().await.clone()
    }

    pub async fn set_settings(&self, s: Settings) {
        // Sync the status view first so the dashboard never renders a source as
        // live-and-failing in the same instant the user just switched it off.
        self.status.write().await.apply_settings(&s);
        *self.settings.write().await = Arc::new(s);
    }

    /// Mutate one source's status under a brief write lock.
    pub async fn with_status<F: FnOnce(&mut crate::status::SourceStatus)>(
        &self,
        source: &str,
        f: F,
    ) {
        let mut g = self.status.write().await;
        f(g.entry(source));
    }

    pub async fn status_snapshot(&self) -> Status {
        self.status.read().await.clone()
    }

    pub async fn profile(&self) -> Arc<crate::ats::Profile> {
        self.profile.read().await.clone()
    }

    /// Re-read the resume into a profile. Called wherever the resume or the
    /// stated years change, so the two can never disagree.
    pub async fn rebuild_profile(&self, s: &Settings) {
        let p = crate::ats::Profile::from_resume(&s.resume, s.years_experience);
        tracing::info!(
            years = ?p.years, level = ?p.level, roles = ?p.roles, stack = p.stack.len(),
            "candidate profile rebuilt"
        );
        *self.profile.write().await = Arc::new(p);
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
