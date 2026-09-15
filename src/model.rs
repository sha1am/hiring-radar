use crate::settings::Settings;
use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since the Unix epoch. The whole system uses i64 unix time to keep
/// SQLite mapping trivial and avoid a datetime dependency.
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// A post as pulled from a source, before it becomes a scored candidate.
#[derive(Clone, Debug)]
pub struct RawPost {
    pub source: String,
    pub external_id: String,
    pub url: String,
    pub title: String,
    pub company: String,
    pub location: Option<String>,
    pub body: String,
    pub posted_at: Option<i64>,
    pub apply: ApplyChannel,
    /// True when `title` was invented by the source rather than reported by it.
    /// A feed post has no title field, so we use its first line — matching
    /// target job titles against that is close to meaningless, and the scorer
    /// redistributes the title budget rather than handing out zeros.
    pub synthetic_title: bool,
}

impl RawPost {
    /// Stable dedup key across restarts: hash of source + external id.
    pub fn urn(&self) -> String {
        let mut h = blake3::Hasher::new();
        h.update(self.source.as_bytes());
        h.update(b"|");
        h.update(self.external_id.as_bytes());
        h.finalize().to_hex().to_string()
    }

    pub fn haystack(&self) -> String {
        format!("{} {} {}", self.title, self.company, self.body).to_lowercase()
    }
}

/// Where an application actually goes — decides how we draft and how we send.
#[derive(Clone, Debug)]
pub enum ApplyChannel {
    Email(String),
    ExternalUrl(String),
    LinkedInDm { profile_url: String },
    Unknown,
}

impl ApplyChannel {
    pub fn kind(&self) -> &'static str {
        match self {
            ApplyChannel::Email(_) => "email",
            ApplyChannel::ExternalUrl(_) => "external",
            ApplyChannel::LinkedInDm { .. } => "dm",
            ApplyChannel::Unknown => "unknown",
        }
    }
    pub fn target(&self) -> Option<String> {
        match self {
            ApplyChannel::Email(e) => Some(e.clone()),
            ApplyChannel::ExternalUrl(u) => Some(u.clone()),
            ApplyChannel::LinkedInDm { profile_url } => Some(profile_url.clone()),
            ApplyChannel::Unknown => None,
        }
    }
    pub fn from_parts(kind: &str, target: Option<String>) -> Self {
        match (kind, target) {
            ("email", Some(t)) => ApplyChannel::Email(t),
            ("external", Some(t)) => ApplyChannel::ExternalUrl(t),
            ("dm", Some(t)) => ApplyChannel::LinkedInDm { profile_url: t },
            _ => ApplyChannel::Unknown,
        }
    }
}

/// Release tier derived purely from match score.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Exceptional,
    Strong,
    Marginal,
}

impl Tier {
    /// `None` means the post is below the floor and should be dropped entirely.
    pub fn from_score(score: f64, cfg: &Settings) -> Option<Tier> {
        if score >= cfg.exceptional_min {
            Some(Tier::Exceptional)
        } else if score >= cfg.strong_min {
            Some(Tier::Strong)
        } else if score >= cfg.score_floor {
            Some(Tier::Marginal)
        } else {
            None
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            Tier::Exceptional => "exceptional",
            Tier::Strong => "strong",
            Tier::Marginal => "marginal",
        }
    }
    /// How long a freshly-detected post of this tier holds before it may fire.
    /// Exceptional posts never settle — being first is the whole point.
    pub fn settle_secs(&self, cfg: &Settings) -> i64 {
        match self {
            Tier::Exceptional => 0,
            Tier::Strong => cfg.settle_strong_secs,
            Tier::Marginal => i64::MAX / 4, // effectively never via the instant path
        }
    }
    pub fn ntfy_priority(&self) -> &'static str {
        match self {
            Tier::Exceptional => "urgent",
            Tier::Strong => "high",
            Tier::Marginal => "default",
        }
    }
}

/// A row in the `candidates` table. Timestamps are unix seconds.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct Candidate {
    pub id: i64,
    pub urn: String,
    pub source: String,
    pub url: String,
    pub title: String,
    pub company: String,
    pub location: Option<String>,
    pub body: String,
    pub score: f64,
    /// Priority at INSERT time, which is when age is always ~0 — so it equals
    /// `score` for every row and decays for none. Nothing reads it: selection
    /// calls `live_priority()` instead. Kept as a coarse index for the
    /// pre-filter, and named here so the next person doesn't wire it back into
    /// an ORDER BY.
    #[allow(dead_code)]
    pub priority: f64,
    pub tier: String,
    pub status: String,
    pub detected_at: i64,
    pub posted_at: Option<i64>,
    /// Written by the pipeline and read by the release engine's SQL, not by
    /// Rust — hence unread here.
    #[allow(dead_code)]
    pub expires_at: i64,
    #[allow(dead_code)]
    pub settle_until: i64,
    pub apply_kind: String,
    pub apply_target: Option<String>,
    pub draft_subject: Option<String>,
    pub draft_body: Option<String>,
    #[allow(dead_code)]
    pub notified_at: Option<i64>,
    /// The resume terms that drove this match, for "why did this fire".
    pub match_terms: Option<String>,
    /// Technologies detected in the post, comma-delimited with leading and
    /// trailing commas so a SQL containment test cannot prefix-match.
    pub tags: Option<String>,

    // ---- structured facts (see enrich.rs) ----
    // Written by the heuristics at ingest and refined by the LLM pass. Columns
    // rather than a JSON blob because each one is something you filter by.
    pub role: Option<String>,
    pub level: Option<String>,
    pub years_min: Option<i64>,
    pub years_max: Option<i64>,
    pub work_mode: Option<String>,
    pub employment: Option<String>,
    /// Coarse geography — see `geo::REGIONS`. NULL when the location could not
    /// be resolved, which is never the same as "somewhere else".
    pub region: Option<String>,
    // ---- the ATS assessment (see ats.rs) ----
    // Stored rather than recomputed, so the number the release engine acted on
    // is the number the dashboard shows.
    pub verdict: Option<String>,
    pub reason: Option<String>,
    pub missing: Option<String>,
    /// The per-dimension breakdown, as JSON. Kept so the score can show its
    /// working rather than asking to be trusted.
    pub dimensions: Option<String>,
    /// NULL until the LLM pass has read this row. Doubles as the work queue.
    pub enriched_at: Option<i64>,
}

impl Candidate {
    /// The structured facts, as the enricher's own type.
    pub fn facts(&self) -> crate::enrich::Facts {
        crate::enrich::Facts {
            role: self.role.clone(),
            level: self.level.clone(),
            years_min: self.years_min,
            years_max: self.years_max,
            work_mode: self.work_mode.clone(),
            employment: self.employment.clone(),
            stack: self.tags.as_deref().map(crate::tags::decode).unwrap_or_default(),
        }
    }
}

impl Candidate {
    /// Freshness-weighted rank, evaluated NOW rather than at detection.
    ///
    /// The stored `priority` column is the value at insert time, when age is
    /// always ~0 — so it equals `score` for every row and decays for none.
    /// Ordering by it means an 11-hour-old rollover outranks a minute-old post
    /// of equal match, which is the inversion the tiering exists to prevent.
    /// Selection must therefore recompute; the column is kept only as a coarse
    /// index for the pre-filter.
    pub fn live_priority(&self) -> f64 {
        crate::score::priority(self.score, self.posted_at, self.detected_at, now())
    }

    pub fn apply(&self) -> ApplyChannel {
        ApplyChannel::from_parts(&self.apply_kind, self.apply_target.clone())
    }
    /// Human-friendly "3m ago" style string for the dashboard.
    pub fn age_str(&self) -> String {
        let base = self.posted_at.unwrap_or(self.detected_at);
        let secs = (now() - base).max(0);
        if secs < 90 {
            "just now".into()
        } else if secs < 3600 {
            format!("{}m ago", secs / 60)
        } else if secs < 86400 {
            format!("{}h ago", secs / 3600)
        } else {
            format!("{}d ago", secs / 86400)
        }
    }
}
