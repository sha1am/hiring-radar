use crate::model::now;
use std::collections::BTreeMap;

/// Why the board is empty.
///
/// An empty radar has at least four different causes that look identical from
/// the outside: the first crawl hasn't finished, the source is switched off,
/// every board token 404'd, or everything crawled scored below the floor and
/// was discarded. Without this, the only way to tell them apart is reading
/// container logs, which defeats the point of having a dashboard.
///
/// So the pipeline reports what it did with every post, and sources report what
/// each target returned, and the dashboard says it out loud.

#[derive(Clone, Debug, Default)]
pub struct SourceStatus {
    pub enabled: bool,
    /// Unix seconds of the last completed crawl.
    pub last_run: Option<i64>,
    /// When the next one is due, so the UI can count down rather than say
    /// "soon" to someone watching an empty screen.
    pub next_run: Option<i64>,
    pub running: bool,
    /// True once this source has completed its history-recording first crawl.
    pub bootstrapped: bool,
    /// Whole-source failure (network, etc.) from the last attempt.
    pub last_error: Option<String>,
    /// Per-target outcomes — "stripe: 412 jobs" / "figma: HTTP 404". This is
    /// the line that identifies a mistyped board token, which is otherwise a
    /// completely silent failure.
    pub notes: Vec<crate::sources::Note>,

    // --- last cycle ---
    pub fetched: usize,
    pub new_posts: usize,
    pub stored: usize,
    pub not_hiring: usize,
    pub below_floor: usize,

    // --- cumulative ---
    pub total_fetched: u64,
    pub total_stored: u64,
    pub total_below_floor: u64,
    /// Best score seen from this source, ever. If this sits below your floor,
    /// the floor (or the profile) is the problem, not the source.
    pub best_score: f64,
}

impl SourceStatus {
    pub fn note_outcome(&mut self, o: &Outcome) {
        match o {
            Outcome::Stored { score } => {
                self.stored += 1;
                self.total_stored += 1;
                self.best_score = self.best_score.max(*score);
            }
            Outcome::NotHiring => self.not_hiring += 1,
            Outcome::BelowFloor { score } => {
                self.below_floor += 1;
                self.total_below_floor += 1;
                self.best_score = self.best_score.max(*score);
            }
        }
    }

    /// Reset the per-cycle counters at the top of a crawl.
    pub fn begin_cycle(&mut self) {
        self.running = true;
        self.fetched = 0;
        self.new_posts = 0;
        self.stored = 0;
        self.not_hiring = 0;
        self.below_floor = 0;
        self.notes.clear();
        self.last_error = None;
    }

    pub fn end_cycle(&mut self, next_in_secs: u64) {
        self.running = false;
        self.last_run = Some(now());
        self.next_run = Some(now() + next_in_secs as i64);
    }
}

/// What `pipeline::ingest` decided about one post.
#[derive(Clone, Debug)]
pub enum Outcome {
    Stored { score: f64 },
    NotHiring,
    BelowFloor { score: f64 },
}

#[derive(Clone, Debug, Default)]
pub struct Status {
    pub sources: BTreeMap<String, SourceStatus>,
    pub started_at: i64,
}

impl Status {
    pub fn new() -> Self {
        Self { sources: BTreeMap::new(), started_at: now() }
    }

    pub fn entry(&mut self, name: &str) -> &mut SourceStatus {
        self.sources.entry(name.to_string()).or_default()
    }

    /// Has any source finished a crawl yet?
    pub fn any_crawl_completed(&self) -> bool {
        self.sources.values().any(|s| s.last_run.is_some())
    }

    pub fn any_enabled(&self) -> bool {
        self.sources.values().any(|s| s.enabled)
    }

    pub fn total_stored(&self) -> u64 {
        self.sources.values().map(|s| s.total_stored).sum()
    }

    pub fn total_below_floor(&self) -> u64 {
        self.sources.values().map(|s| s.total_below_floor).sum()
    }

    pub fn total_fetched(&self) -> u64 {
        self.sources.values().map(|s| s.total_fetched).sum()
    }

    pub fn best_score(&self) -> f64 {
        self.sources.values().map(|s| s.best_score).fold(0.0, f64::max)
    }

    /// Targets that explicitly did not deliver.
    pub fn failing_targets(&self) -> Vec<String> {
        self.sources
            .values()
            .filter(|s| s.enabled)
            .flat_map(|s| s.notes.iter())
            .filter(|n| !n.ok)
            .map(|n| n.text.clone())
            .collect()
    }
}

/// One sentence explaining the current state of the board, chosen by the most
/// actionable cause first. Order matters: a misconfigured source is worth
/// saying before "nothing matched", because "nothing matched" is what a broken
/// source looks like.
pub fn diagnosis(st: &Status, floor: f64, rows_in_window: i64) -> (Level, String) {
    if !st.any_enabled() {
        return (
            Level::Warn,
            "No sources are switched on. Enable one in Settings → Sources.".into(),
        );
    }
    if !st.any_crawl_completed() {
        return (
            Level::Info,
            "First crawl is still running. Entries appear as soon as it finishes."
                .into(),
        );
    }

    let failing = st.failing_targets();
    if !failing.is_empty() && st.total_fetched() == 0 {
        return (
            Level::Error,
            format!(
                "Every target failed, so nothing has been crawled: {}. \
                 A wrong Greenhouse board token returns 404 and looks exactly like \
                 an empty board — check the tokens in Settings → Sources.",
                failing.join("; ")
            ),
        );
    }
    if !failing.is_empty() {
        return (
            Level::Warn,
            format!("Some targets are failing: {}.", failing.join("; ")),
        );
    }

    if st.total_fetched() == 0 {
        return (
            Level::Warn,
            "Crawls are succeeding but returning no postings at all. Check your \
             board tokens and LinkedIn queries in Settings → Sources."
                .into(),
        );
    }

    if st.total_stored() == 0 {
        let best = st.best_score();
        return (
            Level::Warn,
            format!(
                "Crawled {} postings, but none scored above your floor of {:.0} \
                 — the best was {:.0}, so everything was discarded. Lower the floor, \
                 widen your target titles, or load your resume so scoring has more \
                 to work with.",
                st.total_fetched(),
                floor,
                best
            ),
        );
    }

    if rows_in_window == 0 {
        return (
            Level::Info,
            format!(
                "{} postings are stored, but none of them were posted inside this \
                 window. Widen the window above.",
                st.total_stored()
            ),
        );
    }

    (
        Level::Ok,
        format!(
            "Watching {} source(s). {} postings crawled, {} kept.",
            st.sources.values().filter(|s| s.enabled).count(),
            st.total_fetched(),
            st.total_stored()
        ),
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Ok,
    Info,
    Warn,
    Error,
}

/// "in 4m 12s" / "2m ago" — a countdown beats "soon" for someone staring at an
/// empty board wondering whether it is broken.
pub fn until(ts: i64) -> String {
    let d = ts - now();
    if d <= 0 {
        return "due now".into();
    }
    if d < 60 {
        format!("in {d}s")
    } else if d < 3600 {
        format!("in {}m {:02}s", d / 60, d % 60)
    } else {
        format!("in {}h {:02}m", d / 3600, (d % 3600) / 60)
    }
}

pub fn since(ts: i64) -> String {
    let d = (now() - ts).max(0);
    if d < 60 {
        format!("{d}s ago")
    } else if d < 3600 {
        format!("{}m ago", d / 60)
    } else {
        format!("{}h ago", d / 3600)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(enabled: bool) -> SourceStatus {
        SourceStatus { enabled, ..Default::default() }
    }

    #[test]
    fn nothing_enabled_is_the_first_thing_said() {
        let mut s = Status::new();
        s.sources.insert("greenhouse".into(), src(false));
        let (lvl, msg) = diagnosis(&s, 55.0, 0);
        assert_eq!(lvl, Level::Warn);
        assert!(msg.contains("No sources"), "{msg}");
    }

    #[test]
    fn before_the_first_crawl_it_says_so() {
        let mut s = Status::new();
        s.sources.insert("greenhouse".into(), src(true));
        let (lvl, msg) = diagnosis(&s, 55.0, 0);
        assert_eq!(lvl, Level::Info);
        assert!(msg.contains("First crawl"), "{msg}");
    }

    /// The case that actually bit: tokens 404, board looks simply empty.
    #[test]
    fn total_target_failure_names_the_tokens() {
        let mut s = Status::new();
        let mut g = src(true);
        g.last_run = Some(now());
        g.notes = vec![crate::sources::Note {
            text: "stripe: HTTP 404 — no such board token".into(),
            ok: false,
        }];
        s.sources.insert("greenhouse".into(), g);
        let (lvl, msg) = diagnosis(&s, 55.0, 0);
        assert_eq!(lvl, Level::Error);
        assert!(msg.contains("404"), "{msg}");
        assert!(msg.contains("token"), "{msg}");
    }

    #[test]
    fn everything_below_floor_reports_the_best_score() {
        let mut s = Status::new();
        let mut g = src(true);
        g.last_run = Some(now());
        g.total_fetched = 120;
        g.total_below_floor = 120;
        g.best_score = 48.0;
        s.sources.insert("greenhouse".into(), g);
        let (lvl, msg) = diagnosis(&s, 55.0, 0);
        assert_eq!(lvl, Level::Warn);
        assert!(msg.contains("48"), "{msg}");
        assert!(msg.contains("55"), "{msg}");
    }

    #[test]
    fn stored_but_outside_the_window_says_widen_it() {
        let mut s = Status::new();
        let mut g = src(true);
        g.last_run = Some(now());
        g.total_fetched = 10;
        g.total_stored = 4;
        s.sources.insert("greenhouse".into(), g);
        let (lvl, msg) = diagnosis(&s, 55.0, 0);
        assert_eq!(lvl, Level::Info);
        assert!(msg.contains("window"), "{msg}");
    }
}
