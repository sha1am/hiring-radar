use crate::config::Config;
use serde::{Deserialize, Serialize};

/// Everything you can change without a rebuild.
///
/// `config.toml` is mounted read-only in the container, so runtime edits cannot
/// write back to it. That is the right shape anyway: the file is the *seed*
/// (and the only place secrets are referenced), and this struct — persisted as
/// one JSON row in SQLite — is the live truth. On boot the row is loaded if it
/// exists, otherwise it is created from the file.
///
/// Adding a field: give it `#[serde(default)]` or a default fn, so an existing
/// stored row without that key still deserialises instead of resetting
/// someone's whole configuration to defaults.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    // ---- who you are / what you want ----
    #[serde(default)]
    pub titles: Vec<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub locations: Vec<String>,
    #[serde(default)]
    pub remote_ok: bool,
    #[serde(default)]
    pub seniority: Vec<String>,
    #[serde(default)]
    pub dealbreakers: Vec<String>,
    #[serde(default)]
    pub min_salary: Option<u64>,

    /// Plain text of your resume. Empty means the lexical scorer stays in
    /// charge; non-empty switches on similarity scoring.
    #[serde(default)]
    pub resume: String,
    #[serde(default)]
    pub resume_filename: Option<String>,
    #[serde(default)]
    pub resume_updated_at: Option<i64>,
    /// 0.0 = ignore the resume, 1.0 = the resume is the entire match signal.
    #[serde(default = "def_resume_weight")]
    pub resume_weight: f64,

    // ---- release tuning ----
    #[serde(default = "def_per_hour_cap")]
    pub per_hour_cap: i64,
    #[serde(default = "def_per_poster_cap")]
    pub per_poster_cap: i64,
    #[serde(default = "def_score_floor")]
    pub score_floor: f64,
    #[serde(default = "def_strong_min")]
    pub strong_min: f64,
    #[serde(default = "def_exceptional_min")]
    pub exceptional_min: f64,
    #[serde(default = "def_settle")]
    pub settle_strong_secs: i64,
    #[serde(default = "def_ttl")]
    pub candidate_ttl_secs: i64,
    #[serde(default)]
    pub adaptive_threshold: bool,
    #[serde(default = "def_adaptive_start")]
    pub adaptive_start: f64,
    #[serde(default = "def_adaptive_end")]
    pub adaptive_end: f64,

    // ---- sources ----
    #[serde(default)]
    pub greenhouse_enabled: bool,
    #[serde(default)]
    pub greenhouse_boards: Vec<String>,
    #[serde(default)]
    pub linkedin_guest_enabled: bool,
    #[serde(default)]
    pub linkedin_queries: Vec<Query>,
    #[serde(default)]
    pub voyager_enabled: bool,

    // ---- dashboard ----
    #[serde(default = "def_radar_hours")]
    pub radar_hours: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Query {
    pub keywords: String,
    pub location: String,
}

fn def_resume_weight() -> f64 { 0.65 }
fn def_per_hour_cap() -> i64 { 4 }
fn def_per_poster_cap() -> i64 { 1 }
fn def_score_floor() -> f64 { 55.0 }
fn def_strong_min() -> f64 { 75.0 }
fn def_exceptional_min() -> f64 { 92.0 }
fn def_settle() -> i64 { 360 }
fn def_ttl() -> i64 { 43200 }
fn def_adaptive_start() -> f64 { 82.0 }
fn def_adaptive_end() -> f64 { 70.0 }
fn def_radar_hours() -> i64 { 24 }

impl Settings {
    /// Seed values from config.toml, used the first time the app runs against
    /// an empty database.
    pub fn from_config(c: &Config) -> Self {
        Self {
            titles: c.profile.titles.clone(),
            keywords: c.profile.keywords.clone(),
            locations: c.profile.locations.clone(),
            remote_ok: c.profile.remote_ok,
            seniority: c.profile.seniority.clone(),
            dealbreakers: c.profile.dealbreakers.clone(),
            min_salary: c.profile.min_salary,

            resume: String::new(),
            resume_filename: None,
            resume_updated_at: None,
            resume_weight: def_resume_weight(),

            per_hour_cap: c.release.per_hour_cap,
            per_poster_cap: c.release.per_poster_cap,
            score_floor: c.release.score_floor,
            strong_min: c.release.strong_min,
            exceptional_min: c.release.exceptional_min,
            settle_strong_secs: c.release.settle_strong_secs,
            candidate_ttl_secs: c.release.candidate_ttl_secs,
            adaptive_threshold: c.release.adaptive_threshold,
            adaptive_start: c.release.adaptive_start,
            adaptive_end: c.release.adaptive_end,

            greenhouse_enabled: c.greenhouse.enabled,
            greenhouse_boards: c.greenhouse.boards.clone(),
            linkedin_guest_enabled: !c.crawl.linkedin_queries.is_empty(),
            linkedin_queries: c
                .crawl
                .linkedin_queries
                .iter()
                .map(|q| Query {
                    keywords: q.keywords.clone(),
                    location: q.location.clone(),
                })
                .collect(),
            voyager_enabled: c.linkedin_voyager.enabled,

            radar_hours: c.server.radar_hours,
        }
    }

    /// Clamp anything a form post could set to a value that breaks the engine —
    /// a zero cap that stops all alerts, a floor above the exceptional bar that
    /// silently drops everything, an inverted tier ladder.
    pub fn sanitize(&mut self) {
        self.per_hour_cap = self.per_hour_cap.clamp(1, 100);
        self.per_poster_cap = self.per_poster_cap.clamp(1, self.per_hour_cap);
        self.score_floor = self.score_floor.clamp(0.0, 99.0);
        self.strong_min = self.strong_min.clamp(self.score_floor + 1.0, 100.0);
        self.exceptional_min = self.exceptional_min.clamp(self.strong_min + 1.0, 100.0);
        self.settle_strong_secs = self.settle_strong_secs.clamp(0, 24 * 3600);
        self.candidate_ttl_secs = self.candidate_ttl_secs.clamp(3600, 30 * 86400);
        self.adaptive_end = self.adaptive_end.clamp(self.score_floor, 100.0);
        self.adaptive_start = self.adaptive_start.clamp(self.adaptive_end, 100.0);
        self.resume_weight = self.resume_weight.clamp(0.0, 1.0);
        self.radar_hours = self.radar_hours.clamp(1, 24 * 30);

        // Free-text lists: drop blanks, trim, lowercase, dedupe. These are all
        // matched case-insensitively, so storing mixed case just hides duplicates.
        for v in [
            &mut self.titles,
            &mut self.keywords,
            &mut self.locations,
            &mut self.seniority,
            &mut self.dealbreakers,
        ] {
            for s in v.iter_mut() {
                *s = s.trim().to_lowercase();
            }
            v.retain(|s| !s.is_empty());
            v.sort();
            v.dedup();
        }

        for b in self.greenhouse_boards.iter_mut() {
            *b = b.trim().to_lowercase();
        }
        self.greenhouse_boards.retain(|b| !b.is_empty());
        self.greenhouse_boards.sort();
        self.greenhouse_boards.dedup();

        self.linkedin_queries
            .retain(|q| !q.keywords.trim().is_empty());

        // A resume bigger than this is not a resume.
        const MAX_RESUME: usize = 200_000;
        if self.resume.len() > MAX_RESUME {
            self.resume.truncate(MAX_RESUME);
        }
    }

    pub fn has_resume(&self) -> bool {
        self.resume.trim().len() > 200
    }
}

/// Parse a textarea where the user puts one entry per line (or a comma list).
pub fn parse_list(raw: &str) -> Vec<String> {
    raw.split(['\n', ','])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}
