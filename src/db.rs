use crate::model::{now, Candidate};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
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
        notified_at INTEGER
    )",
    "CREATE INDEX IF NOT EXISTS idx_candidates_status ON candidates(status, priority DESC)",
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
    "CREATE TABLE IF NOT EXISTS feedback (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        candidate_id INTEGER NOT NULL,
        action TEXT NOT NULL,
        at INTEGER NOT NULL
    )",
];

pub async fn connect(url: &str) -> anyhow::Result<SqlitePool> {
    let opts = SqliteConnectOptions::from_str(url)?
        .create_if_missing(true)
        .busy_timeout(std::time::Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(opts)
        .await?;
    for stmt in SCHEMA {
        sqlx::query(stmt).execute(&pool).await?;
    }
    Ok(pool)
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
}

pub async fn insert_candidate(pool: &SqlitePool, c: &NewCandidate) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO candidates
         (urn, source, url, title, company, location, body, score, priority, tier, status,
          detected_at, posted_at, expires_at, settle_until, apply_kind, apply_target,
          draft_subject, draft_body)
         VALUES (?,?,?,?,?,?,?,?,?,?, 'scored', ?,?,?,?,?,?,?,?)
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
    .bind(c.detected_at)
    .bind(c.posted_at)
    .bind(c.expires_at)
    .bind(c.settle_until)
    .bind(&c.apply_kind)
    .bind(&c.apply_target)
    .bind(&c.draft_subject)
    .bind(&c.draft_body)
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

/// Cards currently awaiting your action on the dashboard.
pub async fn active(pool: &SqlitePool) -> anyhow::Result<Vec<Candidate>> {
    let rows = sqlx::query_as::<_, Candidate>(
        "SELECT * FROM candidates WHERE status = 'notified' ORDER BY notified_at DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Recently actioned items, for the history strip.
pub async fn history(pool: &SqlitePool, limit: i64) -> anyhow::Result<Vec<Candidate>> {
    let rows = sqlx::query_as::<_, Candidate>(
        "SELECT * FROM candidates
         WHERE status IN ('sent','dismissed','applied')
         ORDER BY id DESC LIMIT ?",
    )
    .bind(limit)
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
