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
    "CREATE INDEX IF NOT EXISTS idx_candidates_unenriched ON candidates(enriched_at) WHERE enriched_at IS NULL",
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
          role, level, years_min, years_max, work_mode, employment, region)
         VALUES (?,?,?,?,?,?,?,?,?,?, ?, ?,?,?,?,?,?,?,?,?,?, ?,?,?,?,?,?,?)
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
        "SELECT * FROM candidates
         WHERE status = 'scored' AND tier != 'marginal'
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

    let sql = format!(
        "SELECT * FROM candidates
         WHERE COALESCE(posted_at, detected_at) > ?
           AND (? = '' OR source = ?)
           AND (? = '' OR status = ?)
           AND (? = '' OR tier = ?)
           AND score >= ?
           AND (? = '' OR (
                 lower(title) LIKE ?
              OR lower(company) LIKE ?
              OR lower(COALESCE(location, '')) LIKE ?
              OR lower(COALESCE(match_terms, '')) LIKE ?
           ))
         {tag_clause}{role_clause}{level_clause}{mode_clause}{region_clause}{years_clause}
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
    for v in f.roles.iter().chain(&f.levels).chain(&f.work_modes).chain(&f.regions) {
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
        "INSERT INTO notifications (candidate_id, company, fired_at, channel)
         VALUES (?, ?, ?, 'ntfy+email') ON CONFLICT(candidate_id) DO NOTHING",
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
         WHERE status = 'scored' AND tier = 'marginal' AND expires_at > ?
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
pub async fn set_facts(
    pool: &SqlitePool,
    id: i64,
    f: &crate::enrich::Facts,
    tags: Option<String>,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE candidates
         SET role = ?, level = ?, years_min = ?, years_max = ?,
             work_mode = ?, employment = ?, tags = ?, enriched_at = ?
         WHERE id = ?",
    )
    .bind(&f.role)
    .bind(&f.level)
    .bind(f.years_min)
    .bind(f.years_max)
    .bind(&f.work_mode)
    .bind(&f.employment)
    .bind(tags)
    .bind(now())
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
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
    const ALLOWED: &[&str] = &["role", "level", "work_mode", "employment", "region"];
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
