//! Where jobs come from.
//!
//! One directory per site, because the sites are not alike. Greenhouse is public
//! JSON you can poll freely; Voyager is a private API that will restrict your
//! account; Workday is a per-tenant endpoint with a paging bug you have to work
//! around. A single "fetch some jobs" client good enough for all three would be
//! good at none of them.
//!
//! Each site directory holds the same three pieces, so moving between them is
//! cheap:
//!
//! - **`client`** — the transport. Speaks that site's protocol and wire format,
//!   returns either data or a sentence saying why not.
//! - **`parse`** — normalisation. Wire format in, [`RawPost`] out; no I/O, so it
//!   is testable against a recorded payload.
//! - **`service`** — the crawl strategy, and the [`JobSource`] impl. What to
//!   fetch, in what order, how deep, and what to tell the dashboard.
//!
//! [`common`] holds what they genuinely share, and nothing else.
//!
//! All of this is one binary. The separation is about strategies not leaking
//! into each other, not about deployment: three processes would mean three
//! copies of the database, the scorer and the release engine, to solve a problem
//! nobody has.

pub mod common;
pub mod greenhouse;
pub mod lever;
pub mod linkedin;
pub mod workday;

use crate::model::RawPost;
use crate::settings::Settings;

/// What one crawl produced: the posts, plus a per-target note for each thing it
/// tried. The notes exist because a wrong Greenhouse board token returns a 404
/// that is otherwise completely silent — the board simply looks empty.
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
        self.notes.push(Note {
            text: s.into(),
            ok: true,
        });
    }
    /// A target that did not.
    pub fn fail(&mut self, s: impl Into<String>) {
        self.notes.push(Note {
            text: s.into(),
            ok: false,
        });
    }
    /// Neutral information, e.g. "nothing configured".
    pub fn info(&mut self, s: impl Into<String>) {
        self.notes.push(Note {
            text: s.into(),
            ok: true,
        });
    }
}

/// Every source is a plug: fetch a batch of current posts. The pipeline handles
/// dedup, classification, scoring and release — sources only retrieve and
/// normalise.
#[async_trait::async_trait]
pub trait JobSource: Send + Sync {
    fn name(&self) -> &str;

    /// Sources read their targets from the live settings snapshot rather than
    /// from construction-time config, so boards and queries can be edited in the
    /// dashboard without a restart. A source with nothing configured returns an
    /// empty batch; the crawl loop treats that as a no-op.
    async fn fetch(&self, s: &Settings) -> anyhow::Result<Fetched>;

    /// Whether this source is switched on right now. Separate from fetch() so
    /// the dashboard can distinguish "off" from "on but returning nothing".
    fn is_enabled(&self, s: &Settings) -> bool;
}
