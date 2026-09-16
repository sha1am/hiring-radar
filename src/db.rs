use crate::model::{now, Candidate};
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous,
};
use sqlx::SqlitePool;
use std::str::FromStr;

const SCHEMA: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS candidates (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        urn TEXT NOT NULL UNIQUE,
        source TEXT NOT NULL,
        url TEXT NOT NULL,
        title TEXT NOT NULL,
        company TEXT NOT NULL,
        location TEXT,
        body TEXT NOT NULL,
        score REAL NOT NULL,
        priority REAL NOT NULL,
        tier TEXT NOT NULL,
        status TEXT NOT NULL,
        detected_at INTEGER NOT NULL,
        posted_at INTEGER,
        expires_at INTEGER NOT NULL,
        settle_until INTEGER NOT NULL,
        apply_kind TEXT NOT NULL,
        apply_target TEXT,
        draft_subject TEXT,
        draft_body TEXT,
        notified_at INTEGER,
        match_terms TEXT,
        tags TEXT
    )",
    // Persisted IDF corpus, so a restart doesn't start from a flat idf of 1.
    "CREATE TABLE IF NOT EXISTS corpus_terms (
        term TEXT PRIMARY KEY,
        df INTEGER NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS corpus_meta (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        docs INTEGER NOT NULL
    )",
    "CREATE INDEX IF NOT EXISTS idx_candidates_status ON candidates(status, priority DESC)",
    // The radar view filters on effective post time across every status.
    "CREATE INDEX IF NOT EXISTS idx_candidates_seen_at
       ON candidates(COALESCE(posted_at, detected_at) DESC)",
    // Notification log — the source of truth for the rolling budget + exactly-once.
    "CREATE TABLE IF NOT EXISTS notifications (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        candidate_id INTEGER NOT NULL UNIQUE,
        company TEXT NOT NULL,
        fired_at INTEGER NOT NULL,
        channel TEXT NOT NULL
    )",
    "CREATE INDEX IF NOT EXISTS idx_notifications_fired ON notifications(fired_at)",
    // Every URN we've ever laid eyes on, so restarts never re-notify and the
    // first crawl of a source can be marked backfilled instead of blasted.
    "CREATE TABLE IF NOT EXISTS seen (
        urn TEXT PRIMARY KEY,
        source TEXT NOT NULL,
        first_seen INTEGER NOT NULL,
        backfilled INTEGER NOT NULL DEFAULT 0
    )",
    // Runtime settings: one row, JSON. config.toml is read-only in the
    // container, so this is where live edits land.
    "CREATE TABLE IF NOT EXISTS settings (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        json TEXT NOT NULL,
        updated_at INTEGER NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS feedback (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        candidate_id INTEGER NOT NULL,
        action TEXT NOT NULL,
        at INTEGER NOT NULL
    )",
];

/// Applied on every boot, failures ignored — see connect().
const MIGRATIONS: &[&str] = &[
    "ALTER TABLE candidates ADD COLUMN match_terms TEXT",
    "ALTER TABLE candidates ADD COLUMN tags TEXT",
    // Structured facts, from the heuristics at ingest and refined by the LLM
    // pass. Separate columns rather than a JSON blob because every one of them
    // is something you filter and facet by, and SQLite cannot index into JSON
    // without extracting it on every row.
    "ALTER TABLE candidates ADD COLUMN role TEXT",
    "ALTER TABLE candidates ADD COLUMN level TEXT",
    "ALTER TABLE candidates ADD COLUMN years_min INTEGER",
    "ALTER TABLE candidates ADD COLUMN years_max INTEGER",
    "ALTER TABLE candidates ADD COLUMN work_mode TEXT",
    "ALTER TABLE candidates ADD COLUMN employment TEXT",
    // NULL means the LLM pass has not looked at this row yet. That is the whole
    // work queue — no separate table, and it survives a restart for free.
    "ALTER TABLE candidates ADD COLUMN enriched_at INTEGER",
    // Coarse geography — india / gulf / sea / europe / … See geo::REGIONS.
    // NULL means the location could not be resolved, which is its own case and
    // never means "somewhere else".
    "ALTER TABLE candidates ADD COLUMN region TEXT",
    // The ATS assessment (see ats.rs). Stored rather than recomputed so the
    // number the release engine acted on is the number the dashboard shows.
    "ALTER TABLE candidates ADD COLUMN verdict TEXT",
    "ALTER TABLE candidates ADD COLUMN reason TEXT",
    // Comma-delimited with leading and trailing commas, like tags, so a SQL
    // containment test cannot prefix-match.
    "ALTER TABLE candidates ADD COLUMN missing TEXT",
    "ALTER TABLE candidates ADD COLUMN dimensions TEXT",
    "CREATE INDEX IF NOT EXISTS idx_candidates_unenriched ON candidates(enriched_at) WHERE enriched_at IS NULL",
    // How many times dispatch has claimed this candidate and delivered nothing.
    // The budget is given back each time (see `unfire`), so this is the only
    // thing standing between a wrong SMTP password and the same post being
    // retried on every tick until it expires.
    "ALTER TABLE candidates ADD COLUMN send_failures INTEGER NOT NULL DEFAULT 0",
    // Named in `instant_stack` — see settings. Kept on the row rather than
    // recomputed because it is the reason this posting bypassed the floor and
    // the settling window, and a row should carry the reason it was treated the
    // way it was.
    "ALTER TABLE candidates ADD COLUMN instant INTEGER NOT NULL DEFAULT 0",
    // Everything one model call produces. A call costs money and seconds, and
    // the previous version spent both and then kept seven fields out of the
    // reply — so every field the model answers now has somewhere to live,
    // including the reply itself. `llm_raw` is the only copy of what was
    // actually said: a schema is a guess about what will matter later, and
    // re-reading a stored reply is free where re-asking is not.
    "ALTER TABLE candidates ADD COLUMN must_have TEXT",
    "ALTER TABLE candidates ADD COLUMN nice_to_have TEXT",
    "ALTER TABLE candidates ADD COLUMN responsibilities TEXT",
    "ALTER TABLE candidates ADD COLUMN domain TEXT",
    "ALTER TABLE candidates ADD COLUMN salary_min INTEGER",
    "ALTER TABLE candidates ADD COLUMN salary_max INTEGER",
    "ALTER TABLE candidates ADD COLUMN salary_currency TEXT",
    "ALTER TABLE candidates ADD COLUMN salary_period TEXT",
    "ALTER TABLE candidates ADD COLUMN visa_sponsorship INTEGER",
    "ALTER TABLE candidates ADD COLUMN red_flags TEXT",
    "ALTER TABLE candidates ADD COLUMN summary TEXT",
    "ALTER TABLE candidates ADD COLUMN llm_fit INTEGER",
    "ALTER TABLE candidates ADD COLUMN llm_fit_reason TEXT",
    "ALTER TABLE candidates ADD COLUMN llm_confidence REAL",
    "ALTER TABLE candidates ADD COLUMN llm_model TEXT",
    "ALTER TABLE candidates ADD COLUMN llm_prompt_tokens INTEGER",
    "ALTER TABLE candidates ADD COLUMN llm_completion_tokens INTEGER",
    "ALTER TABLE candidates ADD COLUMN llm_raw TEXT",
];

pub async fn connect(url: &str) -> anyhow::Result<SqlitePool> {
    // WAL matters here: N crawl loops, the release ticker and every web handler
    // write through one pool. In rollback-journal mode a writer also blocks
    // readers, which shows up as a dashboard that stalls mid-crawl.
    let opts = SqliteConnectOptions::from_str(url)?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(std::time::Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(opts)
        .await?;
    for stmt in SCHEMA {
        sqlx::query(stmt).execute(&pool).await?;
    }
    // Additive migrations for databases created before a column existed.
    // SQLite has no ADD COLUMN IF NOT EXISTS, so a duplicate-column error here
    // is the expected "already migrated" case, not a failure.
    for stmt in MIGRATIONS {
        if let Err(e) = sqlx::query(stmt).execute(&pool).await {
            tracing::debug!(%e, stmt, "migration skipped (already applied?)");
        }
    }
    Ok(pool)
}

/// Load the persisted IDF corpus.
pub async fn load_corpus(pool: &SqlitePool) -> anyhow::Result<crate::resume::Corpus> {
    let mut c = crate::resume::Corpus::default();
    let docs: Option<(i64,)> = sqlx::query_as("SELECT docs FROM corpus_meta WHERE id = 1")
        .fetch_optional(pool)
        .await?;
    c.docs = docs.map(|(d,)| d.max(0) as u64).unwrap_or(0);
    let rows: Vec<(String, i64)> = sqlx::query_as("SELECT term, df FROM corpus_terms")
        .fetch_all(pool)
        .await?;
    for (t, df) in rows {
        c.df.insert(t, df.max(0) as u64);
    }
    Ok(c)
}

/// Persist the corpus. Only terms seen at least `min_df` times are written:
/// singletons are most of the map and carry no discriminative weight yet, so
/// storing them would multiply the write for no ranking benefit.
pub async fn save_corpus(
    pool: &SqlitePool,
    c: &crate::resume::Corpus,
    min_df: u64,
) -> anyhow::Result<usize> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO corpus_meta (id, docs) VALUES (1, ?)
         ON CONFLICT(id) DO UPDATE SET docs = excluded.docs",
    )
    .bind(c.docs as i64)
    .execute(&mut *tx)
    .await?;

    let mut written = 0usize;
    for (term, df) in c.df.iter().filter(|(_, d)| **d >= min_df) {
        sqlx::query(
            "INSERT INTO corpus_terms (term, df) VALUES (?, ?)
             ON CONFLICT(term) DO UPDATE SET df = excluded.df",
        )
        .bind(term)
        .bind(*df as i64)
        .execute(&mut *tx)
        .await?;
        written += 1;
    }
    tx.commit().await?;
    Ok(written)
}

/// The stored settings row, or None on a fresh database.
pub async fn load_settings(pool: &SqlitePool) -> anyhow::Result<Option<crate::settings::Settings>> {
    let row: Option<(String,)> = sqlx::query_as("SELECT json FROM settings WHERE id = 1")
        .fetch_optional(pool)
        .await?;
    let Some((json,)) = row else { return Ok(None) };
    match serde_json::from_str(&json) {
        Ok(s) => Ok(Some(s)),
        Err(e) => {
            // A settings row we can't parse must not take the process down —
            // fall back to the config.toml seed and say so loudly.
            tracing::error!(%e, "stored settings unreadable; falling back to config.toml");
            Ok(None)
        }
    }
}

pub async fn save_settings(
    pool: &SqlitePool,
    s: &crate::settings::Settings,
) -> anyhow::Result<()> {
    let json = serde_json::to_string(s)?;
    sqlx::query(
        "INSERT INTO settings (id, json, updated_at) VALUES (1, ?, ?)
         ON CONFLICT(id) DO UPDATE SET json = excluded.json, updated_at = excluded.updated_at",
    )
    .bind(json)
    .bind(now())
    .execute(pool)
    .await?;
    Ok(())
}

/// How many URNs we've already recorded for a source. Zero => first run => bootstrap.
pub async fn seen_count(pool: &SqlitePool, source: &str) -> anyhow::Result<i64> {
    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM seen WHERE source = ?")
        .bind(source)
        .fetch_one(pool)
        .await?;
    Ok(n)
}

/// Records the URN. Returns true if it was newly inserted (i.e. genuinely new to us).
pub async fn record_seen(
    pool: &SqlitePool,
    urn: &str,
    source: &str,
    backfilled: bool,
) -> anyhow::Result<bool> {
    let res = sqlx::query(
        "INSERT INTO seen (urn, source, first_seen, backfilled)
         VALUES (?, ?, ?, ?) ON CONFLICT(urn) DO NOTHING",
    )
    .bind(urn)
    .bind(source)
    .bind(now())
    .bind(backfilled as i64)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

#[allow(clippy::too_many_arguments)]
#[derive(Default)]
pub struct NewCandidate {
    pub urn: String,
    pub source: String,
    pub url: String,
    pub title: String,
    pub company: String,
    pub location: Option<String>,
    pub body: String,
    pub score: f64,
    pub priority: f64,
    pub tier: String,
    pub detected_at: i64,
    pub posted_at: Option<i64>,
    pub expires_at: i64,
    pub settle_until: i64,
    pub apply_kind: String,
    pub apply_target: Option<String>,
    pub draft_subject: Option<String>,
    pub draft_body: Option<String>,
    /// Names a technology from `instant_stack`.
    pub instant: bool,
    pub match_terms: Option<String>,
    pub tags: Option<String>,
    /// The heuristic reading, done at ingest so the filters work immediately.
    /// The LLM pass refines these later; see `enrich`.
    pub role: Option<String>,
    pub level: Option<String>,
    pub years_min: Option<i64>,
    pub years_max: Option<i64>,
    pub work_mode: Option<String>,
    pub employment: Option<String>,
    pub region: Option<String>,
    /// The ATS assessment at ingest. The LLM pass may sharpen it later.
    pub verdict: Option<String>,
    pub reason: Option<String>,
    pub missing: Option<String>,
    pub dimensions: Option<String>,
}

/// `status` is 'scored' for live detections and 'backfilled' for the first
/// crawl of a source. Backfilled rows are visible on the dashboard but are
/// excluded from `eligible()`, so history populates the radar without ever
/// firing a notification.
pub async fn insert_candidate(
    pool: &SqlitePool,
    c: &NewCandidate,
    status: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO candidates
         (urn, source, url, title, company, location, body, score, priority, tier, status,
          detected_at, posted_at, expires_at, settle_until, apply_kind, apply_target,
          draft_subject, draft_body, match_terms, tags,
          role, level, years_min, years_max, work_mode, employment, region,
          verdict, reason, missing, dimensions, instant)
         VALUES (?,?,?,?,?,?,?,?,?,?, ?, ?,?,?,?,?,?,?,?,?,?, ?,?,?,?,?,?,?, ?,?,?,?, ?)
         ON CONFLICT(urn) DO NOTHING",
    )
    .bind(&c.urn)
    .bind(&c.source)
    .bind(&c.url)
    .bind(&c.title)
    .bind(&c.company)
    .bind(&c.location)
    .bind(&c.body)
    .bind(c.score)
    .bind(c.priority)
    .bind(&c.tier)
    .bind(status)
    .bind(c.detected_at)
    .bind(c.posted_at)
    .bind(c.expires_at)
    .bind(c.settle_until)
    .bind(&c.apply_kind)
    .bind(&c.apply_target)
    .bind(&c.draft_subject)
    .bind(&c.draft_body)
    .bind(&c.match_terms)
    .bind(&c.tags)
    .bind(&c.role)
    .bind(&c.level)
    .bind(c.years_min)
    .bind(c.years_max)
    .bind(&c.work_mode)
    .bind(&c.employment)
    .bind(&c.region)
    .bind(&c.verdict)
    .bind(&c.reason)
    .bind(&c.missing)
    .bind(&c.dimensions)
    .bind(c.instant)
    .execute(pool)
    .await?;
    Ok(())
}

/// Expire anything past its TTL that never made it out.
pub async fn expire_stale(pool: &SqlitePool) -> anyhow::Result<u64> {
    let res = sqlx::query(
        "UPDATE candidates SET status = 'expired'
         WHERE status = 'scored' AND expires_at <= ?",
    )
    .bind(now())
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

/// Notifications fired in the trailing hour = spent budget.
pub async fn budget_used(pool: &SqlitePool) -> anyhow::Result<i64> {
    let (n,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM notifications WHERE fired_at > ?")
            .bind(now() - 3600)
            .fetch_one(pool)
            .await?;
    Ok(n)
}

pub async fn poster_used(pool: &SqlitePool, company: &str) -> anyhow::Result<i64> {
    let (n,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM notifications WHERE company = ? AND fired_at > ?",
    )
    .bind(company)
    .bind(now() - 3600)
    .fetch_one(pool)
    .await?;
    Ok(n)
}

/// Eligible pool for the release engine, best first. Marginal tier is excluded
/// from the instant path; it only leaves via the hourly digest.
pub async fn eligible(pool: &SqlitePool) -> anyhow::Result<Vec<Candidate>> {
    let rows = sqlx::query_as::<_, Candidate>(
        // `instant = 1` is the second way in: a posting naming something on the
        // watch list is released whatever it scored, because you asked for it
        // by name rather than by fit.
        "SELECT * FROM candidates
         WHERE status = 'scored' AND (tier != 'marginal' OR instant = 1)
           AND settle_until <= ? AND expires_at > ?
         ORDER BY priority DESC",
    )
    .bind(now())
    .bind(now())
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// What the dashboard is asking the radar to show.
///
/// Filtering happens in SQL rather than over the already-rendered list: the
/// query is capped at a few hundred rows, so filtering after the cap would
/// search only the newest slice and quietly miss matches further back in the
/// window.
#[derive(Debug, Default, Clone)]
pub struct RadarFilter {
    /// Free text over title, company, location and the matched resume terms.
    pub q: String,
    pub source: String,
    pub status: String,
    pub tier: String,
    pub min_score: f64,
    /// Selected tech chips. These OR together: chips are a net for scanning,
    /// not a sieve — picking Go and Rust should widen the board to both.
    pub tags: Vec<String>,

    // ---- structured facts (see enrich.rs) ----
    // These AND with everything else but OR within themselves, for the same
    // reason as the tags: picking "backend" and "sre" means "either of those",
    // and the intersection would always be empty.
    pub roles: Vec<String>,
    pub levels: Vec<String>,
    pub work_modes: Vec<String>,
    pub regions: Vec<String>,
    pub verdicts: Vec<String>,
    /// "how much experience are they asking for" as a band you can sit inside.
    /// A listing with no stated years passes both, because filtering it out
    /// would be filtering on how the description was written.
    pub years_max_wanted: Option<i64>,
    pub years_min_wanted: Option<i64>,

    pub sort: Sort,
}

/// How the radar list is ordered.
///
/// Two questions, and they want opposite orderings. "What just landed" is a
/// feed — you read it newest first and stop when you reach what you saw last
/// time. "What is worth my afternoon" is a ranking, and the best match from six
/// hours ago beats a fresh mediocre one. Defaulting to either and offering no
/// choice makes the other question unanswerable.
///
/// Every variant orders entirely in SQL, so what you see is the true top N of
/// the whole window rather than the window's first N re-sorted. Sorting a
/// LIMITed page in the client looks identical and is quietly wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    /// Most recently posted first.
    #[default]
    Newest,
    /// Oldest first — for working back through a backlog without losing
    /// your place as new things arrive at the other end.
    Oldest,
    /// Highest match score first.
    Score,
}

impl Sort {
    pub fn parse(s: &str) -> Sort {
        match s.trim().to_lowercase().as_str() {
            "oldest" => Sort::Oldest,
            "score" => Sort::Score,
            _ => Sort::Newest,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Sort::Newest => "newest",
            Sort::Oldest => "oldest",
            Sort::Score => "score",
        }
    }

    /// The ORDER BY clause. Not user input — this is a fixed string chosen by a
    /// closed enum, which is why interpolating it is safe.
    ///
    /// Every ordering ends in `id` so it is total. Without that, rows with the
    /// same score (or the same timestamp, which is common — a board publishes a
    /// dozen listings in the same second) come back in whatever order SQLite
    /// felt like, and shuffle between refreshes under someone's cursor.
    fn clause(&self) -> &'static str {
        match self {
            Sort::Newest => "COALESCE(posted_at, detected_at) DESC, id DESC",
            Sort::Oldest => "COALESCE(posted_at, detected_at) ASC, id ASC",
            Sort::Score => "score DESC, COALESCE(posted_at, detected_at) DESC, id DESC",
        }
    }
}

impl RadarFilter {
}

/// The window, narrowed by whatever the dashboard controls are set to.
pub async fn recent_filtered(
    pool: &SqlitePool,
    window_secs: i64,
    f: &RadarFilter,
    limit: i64,
) -> anyhow::Result<Vec<Candidate>> {
    let cutoff = now() - window_secs;
    let like = format!("%{}%", f.q.to_lowercase());

    // The tag clause is the one part that cannot be a fixed string: it is an OR
    // over however many chips are selected. Built with placeholders and bound
    // below — never interpolated, since these values arrive from a query string.
    let tag_clause = if f.tags.is_empty() {
        String::new()
    } else {
        let ors = f
            .tags
            .iter()
            .map(|_| "instr(COALESCE(tags, ''), ?) > 0")
            .collect::<Vec<_>>()
            .join(" OR ");
        format!(" AND ({ors})")
    };

    // One IN (?,?,?) clause per selected fact, built from placeholders and
    // bound — never interpolated. The column names come from this file, the
    // values come from a URL.
    let in_clause = |col: &str, vals: &[String]| -> String {
        if vals.is_empty() {
            String::new()
        } else {
            let marks = vec!["?"; vals.len()].join(",");
            format!(" AND {col} IN ({marks})")
        }
    };
    let role_clause = in_clause("role", &f.roles);
    let level_clause = in_clause("level", &f.levels);
    let mode_clause = in_clause("work_mode", &f.work_modes);
    let region_clause = in_clause("region", &f.regions);
    let verdict_clause = in_clause("verdict", &f.verdicts);

    // A listing that states no years passes either bound. Dropping those would
    // be filtering on how the description was written, not on the job.
    let years_clause = {
        let mut c = String::new();
        if f.years_max_wanted.is_some() {
            c.push_str(" AND (years_min IS NULL OR years_min <= ?)");
        }
        if f.years_min_wanted.is_some() {
            c.push_str(" AND (years_max IS NULL OR years_max >= ?)");
        }
        c
    };

    // Dismissing is the one status that is a decision rather than a state a
    // row arrived in: it means "I have dealt with this, stop showing it to me".
    // Leaving those rows on the board makes the dismiss button look like it did
    // nothing. They stay reachable by asking for them in the status filter.
    let sql = format!(
        "SELECT * FROM candidates
         WHERE COALESCE(posted_at, detected_at) > ?
           AND (? = '' OR source = ?)
           AND (? = '' OR status = ?)
           AND (status != 'dismissed' OR ? = 'dismissed')
           AND (? = '' OR tier = ?)
           AND score >= ?
           AND (? = '' OR (
                 lower(title) LIKE ?
              OR lower(company) LIKE ?
              OR lower(COALESCE(location, '')) LIKE ?
              OR lower(COALESCE(match_terms, '')) LIKE ?
           ))
         {tag_clause}{role_clause}{level_clause}{mode_clause}{region_clause}{verdict_clause}{years_clause}
         ORDER BY {order}
         LIMIT ?",
        order = f.sort.clause()
    );

    let mut query = sqlx::query_as::<_, Candidate>(&sql)
    .bind(cutoff)
    .bind(&f.source)
    .bind(&f.source)
    .bind(&f.status)
    .bind(&f.status)
    .bind(&f.status)
    .bind(&f.tier)
    .bind(&f.tier)
    .bind(f.min_score)
    .bind(&f.q)
    .bind(&like)
    .bind(&like)
    .bind(&like)
    .bind(&like);

    for t in &f.tags {
        query = query.bind(crate::tags::needle(t));
    }
    // Bound in the same order the clauses were appended above.
    for v in f.roles.iter().chain(&f.levels).chain(&f.work_modes).chain(&f.regions).chain(&f.verdicts) {
        query = query.bind(v.clone());
    }
    if let Some(n) = f.years_max_wanted {
        query = query.bind(n);
    }
    if let Some(n) = f.years_min_wanted {
        query = query.bind(n);
    }

    let rows = query.bind(limit).fetch_all(pool).await?;
    Ok(rows)
}

/// Every tech tag present in the window, with how many posts carry it, most
/// common first. The chips are built from this so they only ever offer
/// something that can return a result.
pub async fn tag_facets(pool: &SqlitePool, window_secs: i64) -> anyhow::Result<Vec<(String, i64)>> {
    let cutoff = now() - window_secs;
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT tags FROM candidates
         WHERE COALESCE(posted_at, detected_at) > ? AND tags IS NOT NULL AND tags != ''",
    )
    .bind(cutoff)
    .fetch_all(pool)
    .await?;

    let mut counts: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for (t,) in rows {
        for tag in crate::tags::decode(&t) {
            *counts.entry(tag).or_insert(0) += 1;
        }
    }
    let mut out: Vec<(String, i64)> = counts.into_iter().collect();
    // Count first, then name, so the order is stable between refreshes rather
    // than shuffling chips under the cursor.
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    Ok(out)
}

/// The distinct sources and statuses actually present in the window, so the
/// filter dropdowns offer only choices that can return something.
pub async fn radar_facets(
    pool: &SqlitePool,
    window_secs: i64,
) -> anyhow::Result<(Vec<String>, Vec<String>)> {
    let cutoff = now() - window_secs;
    let sources: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT source FROM candidates
         WHERE COALESCE(posted_at, detected_at) > ? ORDER BY source",
    )
    .bind(cutoff)
    .fetch_all(pool)
    .await?;
    let statuses: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT status FROM candidates
         WHERE COALESCE(posted_at, detected_at) > ? ORDER BY status",
    )
    .bind(cutoff)
    .fetch_all(pool)
    .await?;
    Ok((
        sources.into_iter().map(|(s,)| s).collect(),
        statuses.into_iter().map(|(s,)| s).collect(),
    ))
}

/// How many rows the radar is holding, for the dashboard counter.
pub async fn recent_count(pool: &SqlitePool, window_secs: i64) -> anyhow::Result<i64> {
    let (n,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM candidates WHERE COALESCE(posted_at, detected_at) > ?",
    )
    .bind(now() - window_secs)
    .fetch_one(pool)
    .await?;
    Ok(n)
}

/// Drop rows far outside any window anyone looks at. `seen` is deliberately
/// NOT pruned — it is what stops a re-crawl from re-notifying, so it has to
/// outlive the candidate row.
pub async fn prune_candidates(pool: &SqlitePool, older_than_secs: i64) -> anyhow::Result<u64> {
    let res = sqlx::query(
        "DELETE FROM candidates
         WHERE COALESCE(posted_at, detected_at) < ?
           AND status NOT IN ('notified','sent','applied')",
    )
    .bind(now() - older_than_secs)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

/// Delete rows the location gate would reject today.
///
/// The gate at ingest only ever looks at arrivals, so switching "only India"
/// on leaves every posting that was stored while it was off — the board stays
/// full of the jobs you just said you did not want, and nothing you can click
/// explains why. This is that switch applied backwards.
///
/// Deleting rather than hiding, because the request is "remove them". Rows you
/// have already acted on are left alone: what you sent or applied to is a
/// record of something you did, not a suggestion to be withdrawn.
pub async fn purge_outside_locations(
    pool: &SqlitePool,
    s: &crate::settings::Settings,
) -> anyhow::Result<u64> {
    if s.location_policy != "require" || s.locations.is_empty() {
        return Ok(0);
    }
    let rows = sqlx::query_as::<_, Candidate>(
        "SELECT * FROM candidates WHERE status NOT IN ('sent', 'applied')",
    )
    .fetch_all(pool)
    .await?;

    // The same function the pipeline uses, on a post rebuilt from the row, so
    // the board can never disagree with the gate about what is in India.
    let doomed: Vec<i64> = rows
        .iter()
        .filter(|c| {
            crate::score::location_verdict(&c.as_post(), s)
                == crate::score::LocationVerdict::Rejected
        })
        .map(|c| c.id)
        .collect();

    let mut removed = 0u64;
    for chunk in doomed.chunks(400) {
        let marks = vec!["?"; chunk.len()].join(",");
        let sql = format!("DELETE FROM candidates WHERE id IN ({marks})");
        let mut q = sqlx::query(&sql);
        for id in chunk {
            q = q.bind(id);
        }
        removed += q.execute(pool).await?.rows_affected();
    }
    Ok(removed)
}

/// Dismiss everything a given board view is showing.
///
/// Deliberately implemented over `recent_filtered` rather than as its own
/// DELETE with its own WHERE clause: the one thing this button must never do is
/// act on a different set of rows than the ones you are looking at, and two
/// copies of a twelve-clause filter would drift apart the first time either was
/// touched.
///
/// Rows already sent, applied to or dismissed are skipped — there is nothing to
/// dismiss about them, and re-stamping them would rewrite their history.
pub async fn dismiss_filtered(
    pool: &SqlitePool,
    window_secs: i64,
    f: &RadarFilter,
    limit: i64,
) -> anyhow::Result<u64> {
    let rows = recent_filtered(pool, window_secs, f, limit).await?;
    let ids: Vec<i64> = rows
        .iter()
        .filter(|c| !matches!(c.status.as_str(), "sent" | "applied" | "dismissed"))
        .map(|c| c.id)
        .collect();

    let mut n = 0u64;
    for chunk in ids.chunks(400) {
        let marks = vec!["?"; chunk.len()].join(",");
        let sql = format!("UPDATE candidates SET status = 'dismissed' WHERE id IN ({marks})");
        let mut q = sqlx::query(&sql);
        for id in chunk {
            q = q.bind(id);
        }
        n += q.execute(pool).await?.rows_affected();
    }
    Ok(n)
}

pub async fn get(pool: &SqlitePool, id: i64) -> anyhow::Result<Option<Candidate>> {
    let row = sqlx::query_as::<_, Candidate>("SELECT * FROM candidates WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

/// Atomically fire: mark notified + log the notification. UNIQUE(candidate_id)
/// guarantees exactly-once even if two ticks race.
pub async fn mark_fired(pool: &SqlitePool, c: &Candidate) -> anyhow::Result<bool> {
    let mut tx = pool.begin().await?;
    let ins = sqlx::query(
        // 'pending' rather than 'ntfy+email': at this point the slot is claimed
        // and nothing has been sent. `record_channel` writes what was actually
        // delivered, and `unfire` removes the row when nothing was.
        "INSERT INTO notifications (candidate_id, company, fired_at, channel)
         VALUES (?, ?, ?, 'pending') ON CONFLICT(candidate_id) DO NOTHING",
    )
    .bind(c.id)
    .bind(&c.company)
    .bind(now())
    .execute(&mut *tx)
    .await?;
    if ins.rows_affected() == 0 {
        tx.rollback().await?;
        return Ok(false);
    }
    sqlx::query("UPDATE candidates SET status = 'notified', notified_at = ? WHERE id = ?")
        .bind(now())
        .bind(c.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}

/// Give back a slot whose dispatch delivered nothing.
///
/// The claim has to happen before the send — it is the `INSERT` that makes
/// firing exactly-once when two ticks race — which means a claim that turns out
/// to be undeliverable has to be handed back, or the notification row stands as
/// a record of something nobody was ever told. That row *is* the hourly budget,
/// and UNIQUE(candidate_id) means it can never fire again: an unset
/// SMTP_PASSWORD was enough to spend every slot, every hour, on silence.
///
/// Returns how many times delivery has now failed for this candidate.
pub async fn unfire(pool: &SqlitePool, id: i64) -> anyhow::Result<i64> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM notifications WHERE candidate_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    // Back to 'scored' is back into `eligible` — the next tick will try again,
    // which is the point: the post is still worth sending.
    sqlx::query(
        "UPDATE candidates
            SET status = 'scored', notified_at = NULL, send_failures = send_failures + 1
          WHERE id = ?",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;
    let (failures,): (i64,) = sqlx::query_as("SELECT send_failures FROM candidates WHERE id = ?")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(failures)
}

/// Record which channels actually delivered.
///
/// The claim is written as 'pending' because at that moment nothing has been
/// sent yet. This is the correction, and it makes the notifications table an
/// honest log of what went out rather than of what was attempted.
pub async fn record_channel(pool: &SqlitePool, id: i64, channel: &str) -> anyhow::Result<()> {
    sqlx::query("UPDATE notifications SET channel = ? WHERE candidate_id = ?")
        .bind(channel)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_draft(
    pool: &SqlitePool,
    id: i64,
    subject: &str,
    body: &str,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE candidates SET draft_subject = ?, draft_body = ? WHERE id = ?")
        .bind(subject)
        .bind(body)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_status(pool: &SqlitePool, id: i64, status: &str) -> anyhow::Result<()> {
    sqlx::query("UPDATE candidates SET status = ? WHERE id = ?")
        .bind(status)
        .bind(id)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO feedback (candidate_id, action, at) VALUES (?, ?, ?)")
        .bind(id)
        .bind(status)
        .bind(now())
        .execute(pool)
        .await?;
    Ok(())
}

/// The outbox: everything worth acting on that you have not acted on yet.
///
/// Independent of the notification budget on purpose. `active()` returns only
/// what actually fired an alert, which is capped at a few per hour — fine as a
/// definition of "what interrupted me", useless as a definition of "what should
/// I apply to". A post that scored 88 on a busy morning is no less worth your
/// time for having missed a slot.
pub async fn outbox(pool: &SqlitePool, min_score: f64, limit: i64) -> anyhow::Result<Vec<Candidate>> {
    let rows = sqlx::query_as::<_, Candidate>(
        "SELECT * FROM candidates
         WHERE score >= ?
           AND status NOT IN ('sent','applied','dismissed','expired')
         ORDER BY score DESC, COALESCE(posted_at, detected_at) DESC
         LIMIT ?",
    )
    .bind(min_score)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn outbox_count(pool: &SqlitePool, min_score: f64) -> anyhow::Result<i64> {
    let (n,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM candidates
         WHERE score >= ? AND status NOT IN ('sent','applied','dismissed','expired')",
    )
    .bind(min_score)
    .fetch_one(pool)
    .await?;
    Ok(n)
}

/// Cards currently awaiting your action on the dashboard.
pub async fn active(pool: &SqlitePool) -> anyhow::Result<Vec<Candidate>> {
    let rows = sqlx::query_as::<_, Candidate>(
        "SELECT * FROM candidates WHERE status = 'notified' ORDER BY notified_at DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn digest_batch(pool: &SqlitePool, limit: i64) -> anyhow::Result<Vec<Candidate>> {
    let rows = sqlx::query_as::<_, Candidate>(
        "SELECT * FROM candidates
         WHERE status = 'scored' AND tier = 'marginal' AND instant = 0
           AND expires_at > ?
         ORDER BY priority DESC LIMIT ?",
    )
    .bind(now())
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn mark_digested(pool: &SqlitePool, ids: &[i64]) -> anyhow::Result<()> {
    for id in ids {
        sqlx::query("UPDATE candidates SET status = 'digested' WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

// ===================== enrichment =====================

/// Rows the LLM pass has not looked at yet, newest first.
///
/// The work queue is a NULL column rather than a separate table: it survives a
/// restart for free, it cannot drift out of sync with the rows it describes, and
/// deleting a candidate deletes its queue entry. Newest first because a backlog
/// is worth clearing in the order you would actually read it — the posting from
/// an hour ago matters more than the one from Tuesday.
pub async fn unenriched(pool: &SqlitePool, limit: i64) -> anyhow::Result<Vec<Candidate>> {
    let rows = sqlx::query_as::<_, Candidate>(
        "SELECT * FROM candidates
         WHERE enriched_at IS NULL
         ORDER BY COALESCE(posted_at, detected_at) DESC
         LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Write back what the model read, and mark the row done.
///
/// `enriched_at` is set even when nothing changed, so a posting the model has no
/// opinion about is not re-read forever.
/// Write back everything one model call produced.
///
/// One statement rather than a fact-shaped one and a model-shaped one, because
/// they describe the same reading of the same posting and a half-applied
/// enrichment is a row nobody can reason about. `enriched_at` is set even when
/// the model had no opinion, so a posting it cannot improve is not retried
/// forever.
pub async fn set_extraction(
    pool: &SqlitePool,
    id: i64,
    f: &crate::enrich::Facts,
    tags: Option<String>,
    e: Option<&crate::enrich::Extraction>,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE candidates
         SET role = ?, level = ?, years_min = ?, years_max = ?,
             work_mode = ?, employment = ?, tags = ?, domain = ?,
             must_have = ?, nice_to_have = ?, responsibilities = ?,
             salary_min = ?, salary_max = ?, salary_currency = ?, salary_period = ?,
             visa_sponsorship = ?, red_flags = ?, summary = ?,
             llm_fit = ?, llm_fit_reason = ?, llm_confidence = ?,
             llm_model = ?, llm_prompt_tokens = ?, llm_completion_tokens = ?,
             llm_raw = ?, enriched_at = ?
         WHERE id = ?",
    )
    .bind(&f.role)
    .bind(&f.level)
    .bind(f.years_min)
    .bind(f.years_max)
    .bind(&f.work_mode)
    .bind(&f.employment)
    .bind(tags)
    .bind(&f.domain)
    .bind(crate::tags::encode(&f.must_have))
    .bind(crate::tags::encode(&f.nice_to_have))
    .bind(crate::tags::encode(&f.responsibilities))
    .bind(e.and_then(|e| e.salary_min))
    .bind(e.and_then(|e| e.salary_max))
    .bind(e.and_then(|e| e.salary_currency.clone()))
    .bind(e.and_then(|e| e.salary_period.clone()))
    .bind(e.and_then(|e| e.visa_sponsorship))
    .bind(e.and_then(|e| crate::tags::encode(&e.red_flags)))
    .bind(e.and_then(|e| e.summary.clone()))
    .bind(e.and_then(|e| e.fit))
    .bind(e.and_then(|e| e.fit_reason.clone()))
    .bind(e.and_then(|e| e.confidence))
    .bind(e.map(|e| e.model.clone()))
    .bind(e.map(|e| e.prompt_tokens))
    .bind(e.map(|e| e.completion_tokens))
    .bind(e.map(|e| e.raw.clone()))
    .bind(now())
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// What the model pass has cost so far, for the status panel.
///
/// This is the one part of the system that is billed per posting. A counter you
/// can see is the difference between an experiment and a surprise.
pub async fn llm_usage(pool: &SqlitePool) -> anyhow::Result<(i64, i64, i64)> {
    let row: (i64, i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM(llm_prompt_tokens), 0),
                COALESCE(SUM(llm_completion_tokens), 0)
         FROM candidates WHERE llm_model IS NOT NULL",
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

/// How many rows are still waiting, for the status panel.
pub async fn unenriched_count(pool: &SqlitePool) -> anyhow::Result<i64> {
    let (n,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM candidates WHERE enriched_at IS NULL")
            .fetch_one(pool)
            .await?;
    Ok(n)
}

/// Distinct values of one enrichment column in the window, with counts.
///
/// Facets have to come from what is actually on the board: offering a "staff"
/// chip when nothing is staff-level is a filter that can only disappoint. The
/// column name is not user input — it comes from a fixed list in the caller —
/// so interpolating it is safe here in a way it would never be for a value.
pub async fn fact_facets(
    pool: &SqlitePool,
    column: &str,
    window_secs: i64,
) -> anyhow::Result<Vec<(String, i64)>> {
    const ALLOWED: &[&str] = &["role", "level", "work_mode", "employment", "region", "verdict"];
    if !ALLOWED.contains(&column) {
        anyhow::bail!("not a facetable column: {column}");
    }
    let cutoff = now() - window_secs;
    let rows: Vec<(String, i64)> = sqlx::query_as(&format!(
        "SELECT {column}, COUNT(*) FROM candidates
         WHERE COALESCE(posted_at, detected_at) > ? AND {column} IS NOT NULL AND {column} != ''
         GROUP BY {column} ORDER BY COUNT(*) DESC"
    ))
    .bind(cutoff)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Fill in `region` for rows that predate the column, or whose location was
/// only worked out later.
///
/// Runs once at boot rather than as a SQL migration, because the mapping lives
/// in the gazetteer and SQLite cannot reach it. Bounded and idempotent: it only
/// touches rows where region is NULL and a location exists, so a second run
/// does nothing.
pub async fn backfill_regions(pool: &SqlitePool) -> anyhow::Result<u64> {
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, COALESCE(location, '') FROM candidates
         WHERE region IS NULL AND COALESCE(location, '') != ''",
    )
    .fetch_all(pool)
    .await?;

    let mut filled = 0u64;
    for (id, location) in rows {
        let Some(region) = crate::geo::region_of(&location) else {
            continue;
        };
        sqlx::query("UPDATE candidates SET region = ? WHERE id = ?")
            .bind(region)
            .bind(id)
            .execute(pool)
            .await?;
        filled += 1;
    }
    Ok(filled)
}

/// What the board says you are most often missing.
///
/// Every assessment records the requirements a posting wanted that you do not
/// have. Across a few hundred postings those aggregate into the one output a
/// job seeker never gets from an ATS: not "you were rejected" but "this is the
/// thing that keeps rejecting you".
///
/// Only genuinely actionable rows count — a posting you already dismissed or
/// applied to is not telling you anything about what to learn next.
pub async fn common_gaps(pool: &SqlitePool, window_secs: i64, limit: usize) -> anyhow::Result<Vec<(String, i64)>> {
    let cutoff = now() - window_secs;
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT missing FROM candidates
         WHERE COALESCE(posted_at, detected_at) > ?
           AND COALESCE(missing, '') != ''
           AND status NOT IN ('dismissed', 'applied', 'sent', 'expired')",
    )
    .bind(cutoff)
    .fetch_all(pool)
    .await?;

    let mut tally: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for (raw,) in rows {
        for item in crate::tags::decode(&raw) {
            // "2 more years" is a fact about one posting, not a gap you can go
            // and close, so it never becomes advice.
            if item.contains("year") || item.contains("level") || item.contains("background") {
                continue;
            }
            *tally.entry(item).or_default() += 1;
        }
    }

    let mut out: Vec<(String, i64)> = tally.into_iter().collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out.truncate(limit);
    Ok(out)
}

/// Rewrite one row's assessment — used when the LLM pass sharpens the facts a
/// posting was originally scored from.
pub async fn set_assessment(
    pool: &SqlitePool,
    id: i64,
    a: &crate::ats::Assessment,
    tier: &str,
    instant: bool,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE candidates
         SET score = ?, tier = ?, verdict = ?, reason = ?, missing = ?, dimensions = ?,
             instant = ?
         WHERE id = ?",
    )
    .bind(a.score)
    .bind(tier)
    .bind(&a.verdict)
    .bind(&a.reason)
    .bind(crate::tags::encode(&a.missing))
    .bind(serde_json::to_string(&a.dimensions).ok())
    .bind(instant)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Every stored candidate, for a re-score.
///
/// Bounded by the prune window rather than by a limit: a partial re-score is
/// worse than none, because the board would then hold two populations scored
/// against different profiles and no way to tell them apart.
pub async fn all_scorable(pool: &SqlitePool) -> anyhow::Result<Vec<Candidate>> {
    let rows = sqlx::query_as::<_, Candidate>(
        "SELECT * FROM candidates
         WHERE status NOT IN ('sent', 'applied', 'dismissed')
         ORDER BY COALESCE(posted_at, detected_at) DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One pool, shared by every test here, created once.
    ///
    /// Not an optimisation: opening a second sqlite pool in the same process
    /// panics inside sqlx 0.7.4 while it builds a row against stale column
    /// metadata. The application opens exactly one pool at startup and keeps it
    /// for the life of the process, so one pool is also the shape being tested.
    ///
    /// Isolation comes from emptying the tables at the start of each test
    /// instead, under a lock that keeps them from interleaving.
    static DB_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    static POOL: tokio::sync::OnceCell<SqlitePool> = tokio::sync::OnceCell::const_new();

    /// Take the lock, get the pool, and start from an empty board.
    async fn fixture() -> (std::sync::MutexGuard<'static, ()>, &'static SqlitePool) {
        let guard = DB_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let pool = POOL
            .get_or_init(|| async {
                let dir = std::env::temp_dir().join(format!(
                    "hiring-radar-test-{}",
                    std::process::id()
                ));
                // WAL leaves -wal and -shm beside the file, and a journal left
                // by an earlier run is replayed into the new database as the
                // schema it was written under.
                let _ = std::fs::remove_dir_all(&dir);
                std::fs::create_dir_all(&dir).expect("test dir");
                connect(&format!("sqlite://{}", dir.join("radar.db").display()))
                    .await
                    .expect("test database")
            })
            .await;
        for t in ["notifications", "candidates"] {
            sqlx::query(&format!("DELETE FROM {t}"))
                .execute(pool)
                .await
                .unwrap();
        }
        (guard, pool)
    }

    async fn candidate(pool: &SqlitePool, urn: &str) -> Candidate {
        insert(pool, urn, "strong", false).await;
        eligible(pool).await.unwrap().pop().expect("one eligible row")
    }

    async fn insert_at(pool: &SqlitePool, urn: &str, location: &str, status: &str) {
        let n = NewCandidate {
            urn: urn.into(),
            source: "greenhouse".into(),
            url: "https://example.com/job".into(),
            title: "Backend Engineer".into(),
            company: "Acme".into(),
            location: Some(location.into()),
            score: 88.0,
            priority: 88.0,
            tier: "strong".into(),
            detected_at: now(),
            expires_at: now() + 3600,
            settle_until: now() - 1,
            apply_kind: "url".into(),
            ..Default::default()
        };
        insert_candidate(pool, &n, status).await.unwrap();
    }

    fn india_only() -> crate::settings::Settings {
        let mut s: crate::settings::Settings = serde_json::from_str("{}").unwrap();
        s.locations = vec!["india".into()];
        s.location_policy = "require".into();
        s
    }

    #[tokio::test]
    async fn switching_on_india_only_clears_what_is_already_there() {
        // The gate at ingest only ever sees arrivals, so without this the board
        // keeps every posting stored while it was off — which is exactly the
        // pile you turned it on to get rid of.
        let (_serial, pool) = fixture().await;
        insert_at(pool, "urn:blr", "Bengaluru, India", "scored").await;
        insert_at(pool, "urn:nyc", "New York, United States", "scored").await;

        let removed = purge_outside_locations(pool, &india_only()).await.unwrap();
        assert_eq!(removed, 1);

        let left: Vec<(String,)> = sqlx::query_as("SELECT urn FROM candidates")
            .fetch_all(pool)
            .await
            .unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].0, "urn:blr");
    }

    #[tokio::test]
    async fn what_you_already_acted_on_is_history_and_survives() {
        // A posting you applied to is a record of something you did, not a
        // suggestion to be withdrawn because the filter changed.
        let (_serial, pool) = fixture().await;
        insert_at(pool, "urn:applied", "Berlin, Germany", "applied").await;
        insert_at(pool, "urn:sent", "Berlin, Germany", "sent").await;
        insert_at(pool, "urn:idle", "Berlin, Germany", "scored").await;

        assert_eq!(purge_outside_locations(pool, &india_only()).await.unwrap(), 1);
        let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM candidates")
            .fetch_one(pool)
            .await
            .unwrap();
        assert_eq!(n, 2);
    }

    #[tokio::test]
    async fn an_advisory_location_list_never_deletes_anything() {
        let (_serial, pool) = fixture().await;
        insert_at(pool, "urn:nyc", "New York, United States", "scored").await;
        let mut s = india_only();
        s.location_policy = "prefer".into();
        assert_eq!(purge_outside_locations(pool, &s).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn a_dismissed_row_leaves_the_board_but_can_be_asked_for() {
        // Otherwise "dismiss all" reads as a button that does nothing: the rows
        // stay exactly where they were, wearing a new word.
        let (_serial, pool) = fixture().await;
        insert_at(pool, "urn:gone", "Bengaluru, India", "dismissed").await;
        insert_at(pool, "urn:here", "Pune, India", "scored").await;

        let visible = recent_filtered(pool, 86_400, &RadarFilter::default(), 50).await.unwrap();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].urn, "urn:here");

        let asked = RadarFilter { status: "dismissed".into(), ..Default::default() };
        let back = recent_filtered(pool, 86_400, &asked, 50).await.unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].urn, "urn:gone");
    }

    #[tokio::test]
    async fn dismiss_all_takes_the_rows_the_filter_shows_and_no_others() {
        let (_serial, pool) = fixture().await;
        insert_at(pool, "urn:a", "Bengaluru, India", "scored").await;
        insert_at(pool, "urn:b", "Pune, India", "scored").await;
        insert_at(pool, "urn:c", "Bengaluru, India", "applied").await;

        // Narrowed to Pune: one row dismissed, and the applied row untouched
        // even though it matches nothing about the filter either way.
        let f = RadarFilter { q: "pune".into(), ..Default::default() };
        assert_eq!(dismiss_filtered(pool, 86_400, &f, 500).await.unwrap(), 1);

        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT urn, status FROM candidates ORDER BY urn")
                .fetch_all(pool)
                .await
                .unwrap();
        assert_eq!(rows[0], ("urn:a".into(), "scored".into()));
        assert_eq!(rows[1], ("urn:b".into(), "dismissed".into()));
        assert_eq!(rows[2], ("urn:c".into(), "applied".into()));
    }

    async fn insert(pool: &SqlitePool, urn: &str, tier: &str, instant: bool) {
        let n = NewCandidate {
            urn: urn.into(),
            source: "greenhouse".into(),
            url: "https://example.com/job".into(),
            title: "Backend Engineer".into(),
            company: "Acme".into(),
            score: 88.0,
            priority: 88.0,
            tier: tier.into(),
            instant,
            detected_at: now(),
            expires_at: now() + 3600,
            settle_until: now() - 1,
            apply_kind: "url".into(),
            ..Default::default()
        };
        insert_candidate(pool, &n, "scored").await.unwrap();
    }

    #[tokio::test]
    async fn a_marginal_posting_naming_something_you_watch_for_still_gets_released() {
        // "Tell me about every Go job" and "show me the best jobs" are
        // different requests. The tier answers the second; without this, a Go
        // role that scored badly on level or years would sit in the digest
        // until tomorrow morning, which is not what "instant" means.
        let (_serial, pool) = fixture().await;
        insert(pool, "urn:instant", "marginal", true).await;
        insert(pool, "urn:quiet", "marginal", false).await;

        let out = eligible(pool).await.unwrap();
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].urn, "urn:instant");

        // And it is not also queued for the hourly digest, which would send it
        // twice.
        let digest = digest_batch(pool, 10).await.unwrap();
        assert_eq!(digest.len(), 1);
        assert_eq!(digest[0].urn, "urn:quiet");
    }

    #[tokio::test]
    async fn a_claim_that_delivered_nothing_gives_the_slot_back() {
        // The finding this was written for: `mark_fired` writes the row that
        // *is* the hourly budget before anything is sent, and a wrong SMTP
        // password used to leave it there — a post marked notified that nobody
        // was told about, which UNIQUE(candidate_id) then made unrepeatable.
        let (_serial, pool) = fixture().await;
        let c = candidate(pool, "urn:1").await;

        assert!(mark_fired(pool, &c).await.unwrap());
        assert_eq!(budget_used(pool).await.unwrap(), 1);

        let failures = unfire(pool, c.id).await.unwrap();
        assert_eq!(failures, 1);
        assert_eq!(budget_used(pool).await.unwrap(), 0, "the slot must come back");

        // And it is eligible again, so the next tick actually retries it.
        let again = eligible(pool).await.unwrap();
        assert_eq!(again.len(), 1);
        assert!(mark_fired(pool, &again[0]).await.unwrap(), "must be re-sendable");
    }

    #[tokio::test]
    async fn a_delivered_send_records_the_channel_that_worked() {
        // 'ntfy+email' was written before either had been attempted, so the
        // table recorded intent and called it delivery.
        let (_serial, pool) = fixture().await;
        let c = candidate(pool, "urn:2").await;
        mark_fired(pool, &c).await.unwrap();

        let (before,): (String,) =
            sqlx::query_as("SELECT channel FROM notifications WHERE candidate_id = ?")
                .bind(c.id)
                .fetch_one(pool)
                .await
                .unwrap();
        assert_eq!(before, "pending");

        record_channel(pool, c.id, "ntfy").await.unwrap();
        let (after,): (String,) =
            sqlx::query_as("SELECT channel FROM notifications WHERE candidate_id = ?")
                .bind(c.id)
                .fetch_one(pool)
                .await
                .unwrap();
        assert_eq!(after, "ntfy", "only the channel that worked");
        assert_eq!(budget_used(pool).await.unwrap(), 1, "a real send spends a slot");
    }

    #[tokio::test]
    async fn repeated_failures_accumulate_so_the_retry_can_be_given_up_on() {
        let (_serial, pool) = fixture().await;
        candidate(pool, "urn:3").await;
        for expected in 1..=3 {
            let c = eligible(pool).await.unwrap().pop().unwrap();
            mark_fired(pool, &c).await.unwrap();
            assert_eq!(unfire(pool, c.id).await.unwrap(), expected);
        }
    }
}
