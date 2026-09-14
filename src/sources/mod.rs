pub mod greenhouse;
pub mod linkedin_guest;
pub mod linkedin_voyager;

use crate::model::RawPost;
use crate::settings::Settings;

/// What one crawl produced: the posts, plus a per-target note for each thing
/// it tried. The notes exist because a wrong Greenhouse board token returns a
/// 404 that is otherwise completely silent — the board simply looks empty.
#[derive(Debug, Clone)]
pub struct Note {
    pub text: String,
    /// False means this target did not deliver. The dashboard colours by this
    /// and the diagnosis counts it — neither should be inferring failure by
    /// pattern-matching the message text.
    pub ok: bool,
}

#[derive(Debug, Default)]
pub struct Fetched {
    pub posts: Vec<RawPost>,
    pub notes: Vec<Note>,
}

impl Fetched {
    /// A target that worked.
    pub fn ok(&mut self, s: impl Into<String>) {
        self.notes.push(Note { text: s.into(), ok: true });
    }
    /// A target that did not.
    pub fn fail(&mut self, s: impl Into<String>) {
        self.notes.push(Note { text: s.into(), ok: false });
    }
    /// Neutral information, e.g. "nothing configured".
    pub fn info(&mut self, s: impl Into<String>) {
        self.notes.push(Note { text: s.into(), ok: true });
    }
}

/// reqwest errors stringify into a paragraph. Keep the cause, drop the URL
/// echo and the nested "error trying to connect:" chain.
pub fn brief(e: &impl std::fmt::Display) -> String {
    let s = e.to_string();
    let tail = s.rsplit(':').next().unwrap_or(&s).trim().to_string();
    if tail.is_empty() { s } else { tail }
}

/// Every source is a plug: fetch a batch of current posts. The pipeline handles
/// dedup, classification, scoring and release — sources only retrieve + normalise.
#[async_trait::async_trait]
pub trait JobSource: Send + Sync {
    fn name(&self) -> &str;
    /// Sources read their targets from the live settings snapshot rather than
    /// from construction-time config, so boards and queries can be edited in
    /// the dashboard without a restart. A source with nothing configured
    /// returns an empty batch; the crawl loop treats that as a no-op.
    async fn fetch(&self, s: &Settings) -> anyhow::Result<Fetched>;

    /// Whether this source is switched on right now. Separate from fetch() so
    /// the dashboard can distinguish "off" from "on but returning nothing".
    fn is_enabled(&self, s: &Settings) -> bool;
}
