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
    /// How much `locations` counts for.
    ///
    /// "prefer" is the original behaviour: a location hit is worth a few points
    /// out of a hundred, which a strong match elsewhere trivially outweighs — so
    /// "only send me jobs in India" was never actually enforced. "require" makes
    /// it a gate: a post that is somewhere else is discarded outright, like a
    /// dealbreaker. "off" ignores location entirely.
    #[serde(default = "def_location_policy")]
    pub location_policy: String,
    /// Under "require", what to do with a post whose location cannot be
    /// determined at all. Feed posts often never name a city, so rejecting
    /// unknowns is defensible but will throw away real matches — hence a
    /// separate switch rather than a hidden assumption.
    #[serde(default = "def_true")]
    pub allow_unknown_location: bool,
    #[serde(default)]
    pub seniority: Vec<String>,
    /// The languages you actually write.
    ///
    /// Separate from `keywords` because keywords are a *bonus* — coverage
    /// across a 38-point content slice — and that is not strong enough to keep
    /// a Ruby job out. A senior backend role in your city collects 62 points
    /// for its title, location and level before anyone asks what language it is
    /// in. This is the gate that asks.
    #[serde(default)]
    pub stack: Vec<String>,
    /// "off" | "prefer" | "require", mirroring `location_policy`.
    ///
    /// "prefer" penalises a post that names only languages you don't write,
    /// which usually drops it under the floor. "require" discards it outright
    /// and says so on the status panel. A post that names no language at all is
    /// never judged either way — plenty of real listings don't.
    #[serde(default = "def_stack_policy")]
    pub stack_policy: String,
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
    /// "all" | "boards" | "posts" | "custom". Anything but "custom" drives the
    /// individual toggles in sanitize(), so the mode is always the truth and
    /// the toggles can't silently disagree with it.
    #[serde(default = "def_mode")]
    pub mode: String,
    #[serde(default)]
    pub greenhouse_enabled: bool,
    /// Fallback only. `companies/greenhouse.txt` is the authority when it
    /// exists; this is what an install that predates the file still runs on.
    #[serde(default)]
    pub greenhouse_boards: Vec<String>,
    #[serde(default)]
    pub workday_enabled: bool,
    /// Fallback for `companies/workday.txt`, same rule as the boards above.
    #[serde(default)]
    pub workday_sites: Vec<String>,
    /// How far back a Workday listing can be and still be worth ingesting.
    ///
    /// Days rather than hours because these are formal requisitions that sit
    /// open for weeks, and because Workday only reports age to the day anyway.
    #[serde(default = "def_workday_lookback_days")]
    pub workday_lookback_days: i64,
    #[serde(default)]
    pub linkedin_guest_enabled: bool,
    #[serde(default)]
    pub linkedin_queries: Vec<Query>,
    #[serde(default)]
    pub voyager_enabled: bool,
    /// Hashtags and phrases to search the LinkedIn feed for, one search each —
    /// "#hiring", "#hiringnow", "we are hiring backend". Separate from the job
    /// titles, because what people write in a hiring POST is not what a job
    /// board puts in a title field.
    #[serde(default = "def_voyager_queries")]
    pub voyager_queries: Vec<String>,
    /// Voyager's queryId. Not a secret — it is a public constant that changes
    /// whenever LinkedIn ships — so it lives here rather than in config.toml,
    /// which is mounted read-only and would need a rebuild to change.
    #[serde(default)]
    pub voyager_query_id: String,
    /// How far back a feed search walks, in hours. This is what makes "every
    /// #hiring post from the last 24 hours" true rather than "the most recent
    /// page of them" — one request returns one page, which on a busy hashtag
    /// can be under an hour.
    #[serde(default = "def_lookback_hours")]
    pub lookback_hours: i64,
    /// Hard ceiling on pages per search per crawl. Pagination multiplies
    /// request volume against an API that bans accounts, so the window is a
    /// target and this is the safety limit.
    #[serde(default = "def_max_pages")]
    pub voyager_max_pages: u32,
    /// Pages per search on every crawl AFTER the window has been filled once.
    /// New posts are on page one, so this stays small — the deep walk is a
    /// one-off, not a heartbeat.
    #[serde(default = "def_poll_pages")]
    pub voyager_poll_pages: u32,

    // ---- dashboard ----
    #[serde(default = "def_radar_hours")]
    pub radar_hours: i64,
    /// Score at which a post is worth acting on, and so lands in the outbox
    /// with a draft.
    ///
    /// Deliberately separate from the alert budget. The 4/hour cap limits how
    /// often your phone buzzes; it should not limit how much you can work
    /// through. Tying the two together means a busy morning silently hides
    /// matches from you.
    #[serde(default = "def_outbox_min")]
    pub outbox_min: f64,
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
fn def_outbox_min() -> f64 { 70.0 }
fn def_mode() -> String { "all".into() }
fn def_location_policy() -> String { "prefer".into() }
fn def_stack_policy() -> String { "off".into() }
fn def_lookback_hours() -> i64 { 24 }
fn def_workday_lookback_days() -> i64 { 7 }
fn def_max_pages() -> u32 { 5 }
fn def_poll_pages() -> u32 { 2 }
fn def_true() -> bool { true }
fn def_voyager_queries() -> Vec<String> {
    vec!["#hiring".into(), "#hiringnow".into(), "#nowhiring".into()]
}

impl Settings {
    /// Seed values from config.toml, used the first time the app runs against
    /// an empty database.
    pub fn from_config(c: &Config) -> Self {
        Self {
            titles: c.profile.titles.clone(),
            keywords: c.profile.keywords.clone(),
            locations: c.profile.locations.clone(),
            remote_ok: c.profile.remote_ok,
            location_policy: def_location_policy(),
            allow_unknown_location: true,
            seniority: c.profile.seniority.clone(),
            stack: Vec::new(),
            stack_policy: def_stack_policy(),
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

            mode: def_mode(),
            greenhouse_enabled: c.greenhouse.enabled,
            greenhouse_boards: c.greenhouse.boards.clone(),
            workday_enabled: true,
            workday_sites: Vec::new(),
            workday_lookback_days: def_workday_lookback_days(),
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
            voyager_queries: def_voyager_queries(),
            voyager_query_id: c.linkedin_voyager.query_id.clone(),
            lookback_hours: def_lookback_hours(),
            voyager_max_pages: def_max_pages(),
            voyager_poll_pages: def_poll_pages(),

            radar_hours: c.server.radar_hours,
            outbox_min: def_outbox_min(),
        }
    }

    /// Clamp anything a form post could set to a value that breaks the engine —
    /// a zero cap that stops all alerts, a floor above the exceptional bar that
    /// silently drops everything, an inverted tier ladder.
    pub fn sanitize(&mut self) {
        // The mode owns the toggles. Without this, switching to "posts" and
        // then flipping a checkbox leaves the UI claiming one thing and the
        // crawl loops doing another.
        match self.mode.as_str() {
            "boards" => {
                self.greenhouse_enabled = true;
                self.workday_enabled = true;
                self.linkedin_guest_enabled = false;
                self.voyager_enabled = false;
            }
            "posts" => {
                self.greenhouse_enabled = false;
                self.workday_enabled = false;
                self.linkedin_guest_enabled = false;
                self.voyager_enabled = true;
            }
            "all" => {
                self.greenhouse_enabled = true;
                self.workday_enabled = true;
                self.linkedin_guest_enabled = true;
                self.voyager_enabled = true;
            }
            _ => self.mode = "custom".into(),
        }

        if !matches!(self.stack_policy.as_str(), "off" | "prefer" | "require") {
            self.stack_policy = def_stack_policy();
        }
        // A gate with nothing behind it would discard everything that names a
        // language, which is not what anyone means by leaving the list empty.
        if self.stack.is_empty() {
            self.stack_policy = "off".into();
        }

        if !matches!(self.location_policy.as_str(), "off" | "prefer" | "require") {
            self.location_policy = def_location_policy();
        }
        // "require" with no locations listed would silently discard everything.
        if self.location_policy == "require" && self.locations.is_empty() {
            self.location_policy = "prefer".into();
        }

        for q in self.voyager_queries.iter_mut() {
            *q = q.trim().to_string();
        }
        self.voyager_queries.retain(|q| !q.is_empty());
        self.voyager_queries.dedup();
        self.voyager_query_id = self.voyager_query_id.trim().to_string();
        self.lookback_hours = self.lookback_hours.clamp(1, 24 * 14);
        self.voyager_max_pages = self.voyager_max_pages.clamp(1, 25);
        self.voyager_poll_pages = self.voyager_poll_pages.clamp(1, self.voyager_max_pages);

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
        self.outbox_min = self.outbox_min.clamp(self.score_floor, 100.0);

        // Free-text lists: drop blanks, trim, lowercase, dedupe. These are all
        // matched case-insensitively, so storing mixed case just hides duplicates.
        for v in [
            &mut self.titles,
            &mut self.keywords,
            &mut self.locations,
            &mut self.seniority,
            &mut self.dealbreakers,
            &mut self.stack,
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

        // Not lowercased, unlike board tokens: a Workday site name is
        // case-sensitive — `NVIDIAExternalCareerSite` 404s as `nvidiaexternal…`.
        for w in self.workday_sites.iter_mut() {
            *w = w.trim().to_string();
        }
        self.workday_sites.retain(|w| !w.is_empty());
        self.workday_sites.dedup();
        self.workday_lookback_days = self.workday_lookback_days.clamp(1, 120);

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

