//! The JSON API.
//!
//! Everything the dashboard does, as data rather than as HTML. The server-rendered
//! pages in `web` stay for now — they are what works today, and replacing the UI
//! and the transport in one step means having nothing that runs while you do it.
//! This module is the seam: the React frontend is written against these shapes,
//! and when it is finished the HTML routes come out.
//!
//! Two rules hold the surface together:
//!
//! - **Anything the UI displays is computed here, not in the client.** Age
//!   strings, tier colours, whether a card can be sent — these depend on
//!   settings and on server time, and a client that recomputes them will drift.
//! - **Every list endpoint returns its facets alongside its rows.** The filter
//!   chips have to know what is *available* to filter by, and a second round
//!   trip to find out means the chips lag the list they filter.

use crate::db;
use crate::model::Candidate;
use crate::settings::Settings;
use crate::state::AppState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/bootstrap", get(bootstrap))
        .route("/api/radar", get(radar))
        .route("/api/queue", get(queue))
        .route("/api/outbox", get(outbox))
        .route("/api/status", get(status))
        .route("/api/settings", get(get_settings).put(put_settings))
        .route("/api/candidate/:id/send", post(send))
        .route("/api/candidate/:id/dismiss", post(dismiss))
        .route("/api/candidate/:id/applied", post(applied))
        .route("/api/candidate/:id/draft", post(save_draft))
        .route(
            "/api/settings/resume",
            // A PDF resume comfortably exceeds axum's 2MB default.
            post(resume_upload).layer(axum::extract::DefaultBodyLimit::max(20 * 1024 * 1024)),
        )
        .route("/api/settings/resume/clear", post(resume_clear))
        .route("/api/logs", get(logs).delete(clear_logs))
}

/// Recent warnings and errors, and the same thing as plain text.
///
/// The text form exists so "send me your logs" is one click rather than a
/// chore — what lands in a report is then formatted identically every time,
/// rather than being whatever the client felt like rendering.
async fn logs(Query(q): Query<HashMap<String, String>>) -> impl IntoResponse {
    let limit = q
        .get("limit")
        .and_then(|l| l.parse::<usize>().ok())
        .unwrap_or(120)
        .clamp(1, 300);
    Json(serde_json::json!({
        "entries": crate::errlog::recent(limit),
        "text": crate::errlog::as_text(limit),
    }))
}

async fn clear_logs() -> impl IntoResponse {
    crate::errlog::clear();
    Json(serde_json::json!({"cleared": true}))
}

/// Upload a resume — PDF, plain text, or pasted.
///
/// Multipart rather than JSON because it carries a file, and the extraction
/// (and its failure modes: a scanned PDF has no text layer) is shared with the
/// HTML endpoint rather than reimplemented.
async fn resume_upload(State(st): State<AppState>, mp: axum::extract::Multipart) -> impl IntoResponse {
    let (text, filename) = match crate::web::read_resume_upload(mp).await {
        Ok(v) => v,
        Err(message) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({"ok": false, "message": message})),
            )
                .into_response()
        }
    };
    match crate::web::store_resume(&st, text, filename).await {
        Ok(message) => {
            st.notify_ui();
            let live = st.settings().await;
            Json(serde_json::json!({
                "ok": true,
                "message": message,
                "settings": settings_dto(&live),
            }))
            .into_response()
        }
        Err(message) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({"ok": false, "message": message})),
        )
            .into_response(),
    }
}

async fn resume_clear(State(st): State<AppState>) -> impl IntoResponse {
    let mut s = (*st.settings().await).clone();
    s.resume.clear();
    s.resume_filename = None;
    s.resume_updated_at = None;
    let _ = db::save_settings(&st.pool, &s).await;
    st.set_settings(s.clone()).await;
    st.rebuild_resume("").await;
    st.rebuild_profile(&s).await;
    if let Err(e) = crate::pipeline::rescore_all(&st).await {
        tracing::warn!(%e, "re-score after resume clear failed");
    }
    st.notify_ui();
    Json(serde_json::json!({
        "ok": true,
        "message": "Resume cleared — scoring is back to your keyword list.",
        "settings": settings_dto(&s),
    }))
}

// ===================== shapes =====================

/// One card, with everything needed to render it and nothing else.
///
/// Deliberately not the database row. `body` alone is often several kilobytes of
/// job description, and sending a hundred of those to paint a list nobody has
/// scrolled yet is the easiest performance mistake to make here — so the list
/// endpoints send `excerpt` and the detail is fetched per card.
#[derive(Serialize, Debug)]
pub struct CardDto {
    pub id: i64,
    pub title: String,
    pub company: String,
    pub location: Option<String>,
    pub url: String,
    pub source: String,
    /// Short label for the source, e.g. `li·posts`. Computed here so two
    /// frontends cannot disagree about what to call a source.
    pub source_label: String,
    pub score: f64,
    /// Freshness-weighted rank as of *now*, which is what the board orders by.
    /// The stored column is the value at insert time and decays for nobody.
    pub priority: f64,
    pub tier: String,
    pub status: String,
    pub posted_at: Option<i64>,
    pub detected_at: i64,
    /// "3m ago". Server-computed because it depends on server time, and a
    /// client clock that is ten minutes out makes every card look stale.
    pub age: String,
    pub tags: Vec<String>,
    /// The resume terms that drove the match — the answer to "why did this fire".
    pub why: Option<String>,
    pub apply_kind: String,
    pub apply_target: Option<String>,
    pub draft_subject: Option<String>,
    pub draft_body: Option<String>,
    pub excerpt: String,

    // ---- structured facts ----
    pub role: Option<String>,
    pub level: Option<String>,
    /// "5+ yrs" / "3–5 yrs", rendered here so two surfaces can't format it
    /// differently.
    pub years: Option<String>,
    pub work_mode: Option<String>,
    pub employment: Option<String>,
    pub region: Option<String>,

    // ---- the ATS assessment (see ats.rs) ----
    /// apply | stretch | reach | skip — advice, not just a number.
    pub verdict: Option<String>,
    /// One sentence naming the dimension that decided the score.
    pub reason: Option<String>,
    /// What the posting wanted that you don't have. The output a candidate
    /// needs and no real ATS ever gives them.
    pub missing: Vec<String>,
    /// The per-dimension breakdown, so the score can show its working rather
    /// than asking to be trusted.
    pub dimensions: Vec<serde_json::Value>,
    /// False while the model still has this row queued.
    pub enriched: bool,
}

impl CardDto {
    pub fn from(c: &Candidate) -> Self {
        Self {
            id: c.id,
            title: c.title.clone(),
            company: c.company.clone(),
            location: c.location.clone(),
            url: c.url.clone(),
            source: c.source.clone(),
            source_label: crate::web::source_label(&c.source).to_string(),
            score: round1(c.score),
            priority: round1(c.live_priority()),
            tier: c.tier.clone(),
            status: c.status.clone(),
            posted_at: c.posted_at,
            detected_at: c.detected_at,
            age: c.age_str(),
            tags: c
                .tags
                .as_deref()
                .map(crate::tags::decode)
                .unwrap_or_default(),
            why: c
                .match_terms
                .clone()
                .filter(|t| !t.trim().is_empty()),
            apply_kind: c.apply_kind.clone(),
            apply_target: c.apply_target.clone(),
            draft_subject: c.draft_subject.clone(),
            draft_body: c.draft_body.clone(),
            excerpt: excerpt(&c.body, 400),
            years: c.facts().years_label(),
            role: c.role.clone(),
            level: c.level.clone(),
            work_mode: c.work_mode.clone(),
            employment: c.employment.clone(),
            region: c.region.clone(),
            verdict: c.verdict.clone(),
            reason: c.reason.clone(),
            missing: c.missing.as_deref().map(crate::tags::decode).unwrap_or_default(),
            dimensions: c
                .dimensions
                .as_deref()
                .and_then(|d| serde_json::from_str(d).ok())
                .unwrap_or_default(),
            enriched: c.enriched_at.is_some(),
        }
    }
}

/// Scores are rendered to one decimal everywhere. Rounding at the boundary
/// means the client never has to decide, and two views can't show 78.4 and 78.35
/// for the same card.
fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// A body preview that ends on a word, not mid-syllable.
fn excerpt(body: &str, max: usize) -> String {
    let body = body.trim();
    if body.chars().count() <= max {
        return body.to_string();
    }
    let cut: String = body.chars().take(max).collect();
    match cut.rfind(char::is_whitespace) {
        Some(i) if i > max / 2 => format!("{}…", &cut[..i]),
        _ => format!("{cut}…"),
    }
}

#[derive(Serialize, Debug)]
pub struct Facet {
    pub value: String,
    /// What the chip should say. `linkedin_voyager` is a key; `li·posts` is a
    /// label; the client should never be turning one into the other.
    pub label: String,
    /// How many rows carry this value, where counting it is cheap. `None` means
    /// "present, not counted" — which is honest, where a 0 or a -1 sentinel
    /// would render as a chip claiming there is nothing behind it.
    pub count: Option<i64>,
}

#[derive(Serialize, Debug)]
pub struct RadarPage {
    pub items: Vec<CardDto>,
    pub window_hours: i64,
    /// Rows in the window before filtering — the denominator for "12 of 340".
    pub total_in_window: i64,
    pub sources: Vec<Facet>,
    pub statuses: Vec<Facet>,
    pub tags: Vec<Facet>,
    /// Structured facets, from what is actually on the board. Offering a
    /// "staff" chip when nothing is staff-level is a filter that can only
    /// disappoint.
    pub roles: Vec<Facet>,
    pub levels: Vec<Facet>,
    pub work_modes: Vec<Facet>,
    /// Coarse geography — india, gulf, sea, europe… Only what's on the board:
    /// a "gulf" chip with nothing behind it is a filter that can only
    /// disappoint.
    pub regions: Vec<Facet>,
    pub verdicts: Vec<Facet>,
    /// What this board keeps asking for that you don't have, most frequent
    /// first. Aggregated from every assessment: not "you were rejected" but
    /// "this is the thing that keeps rejecting you".
    pub gaps: Vec<Facet>,
    /// Echoed back so the client renders the control from what the server
    /// actually applied, not from what it asked for — a typo'd sort silently
    /// falling back to newest while the button still reads "score" is the kind
    /// of disagreement that takes an hour to notice.
    pub sort: String,
    /// How many rows the model hasn't read yet — the facts get sharper as this
    /// drains, and a board that silently changes under you deserves a caption.
    pub pending_enrichment: i64,
}

#[derive(Serialize, Debug)]
pub struct SourceDto {
    pub name: String,
    pub label: String,
    pub enabled: bool,
    pub running: bool,
    pub last_run: Option<i64>,
    pub next_run: Option<i64>,
    pub last_error: Option<String>,
    pub notes: Vec<NoteDto>,
    pub fetched: usize,
    pub new_posts: usize,
    pub stored: usize,
    pub below_floor: usize,
    pub wrong_location: usize,
    pub wrong_stack: usize,
    pub not_hiring: usize,
    pub total_fetched: u64,
    pub total_stored: u64,
    pub best_score: f64,
}

#[derive(Serialize, Debug)]
pub struct NoteDto {
    pub text: String,
    pub ok: bool,
}

#[derive(Serialize, Debug)]
pub struct StatusDto {
    /// "ok" | "info" | "warn" | "error" — the banner's severity.
    pub level: String,
    /// One sentence saying what is wrong and what to do about it.
    pub message: String,
    pub sources: Vec<SourceDto>,
    pub rows_in_window: i64,
    pub window_hours: i64,
    pub outbox_count: i64,
}

/// What a company list looks like from the API: contents, provenance, and the
/// fact that it cannot be edited here.
#[derive(Serialize, Debug)]
pub struct CompanyListDto {
    pub kind: String,
    pub path: String,
    pub entries: Vec<String>,
    /// Always true. Present so the client renders the field disabled because the
    /// server said so, rather than because someone remembered to hard-code it.
    pub read_only: bool,
    /// Why it reads the way it does — "read from X on every crawl", or that the
    /// file is missing and the stored fallback is in use.
    pub note: String,
}

#[derive(Serialize, Debug)]
pub struct SettingsDto {
    #[serde(flatten)]
    pub settings: Settings,
    /// Not part of the settings row: read off disk, and not writable through
    /// this endpoint. Sent alongside so the settings screen is one request.
    pub company_lists: Vec<CompanyListDto>,
    pub has_resume: bool,
    pub resume_chars: usize,
    /// What the resume was actually read as. Shown on the settings page because
    /// the ATS scores against this and not against the PDF — if it read you as
    /// a frontend engineer with two years, every score on the board is wrong
    /// and you would otherwise have no way to find out.
    pub profile: crate::ats::Profile,
}

#[derive(Serialize, Debug)]
pub struct Bootstrap {
    pub status: StatusDto,
    pub queue: Vec<CardDto>,
    pub radar: RadarPage,
    pub outbox: OutboxPage,
}

#[derive(Serialize, Debug)]
pub struct OutboxPage {
    pub items: Vec<CardDto>,
    pub total: i64,
    pub min_score: f64,
}

// ===================== handlers =====================

/// Everything the dashboard needs to paint itself, in one request.
///
/// A first load that fires four parallel requests paints in four stages, each
/// reflowing the page. One request costs the same round trip as the slowest of
/// them and arrives consistent with itself.
async fn bootstrap(State(st): State<AppState>, Query(q): Query<HashMap<String, String>>) -> impl IntoResponse {
    let live = st.settings().await;
    let hours = window_hours(&q, &live);
    Json(Bootstrap {
        status: status_dto(&st, hours).await,
        queue: db::active(&st.pool)
            .await
            .unwrap_or_default()
            .iter()
            .map(CardDto::from)
            .collect(),
        radar: radar_page(&st, hours, &q).await,
        outbox: outbox_page(&st, &live).await,
    })
}

async fn radar(State(st): State<AppState>, Query(q): Query<HashMap<String, String>>) -> impl IntoResponse {
    let live = st.settings().await;
    let hours = window_hours(&q, &live);
    Json(radar_page(&st, hours, &q).await)
}

async fn queue(State(st): State<AppState>) -> impl IntoResponse {
    let items: Vec<CardDto> = db::active(&st.pool)
        .await
        .unwrap_or_default()
        .iter()
        .map(CardDto::from)
        .collect();
    Json(items)
}

async fn outbox(State(st): State<AppState>) -> impl IntoResponse {
    let live = st.settings().await;
    Json(outbox_page(&st, &live).await)
}

async fn status(State(st): State<AppState>) -> impl IntoResponse {
    let hours = st.settings().await.radar_hours;
    Json(status_dto(&st, hours).await)
}

async fn get_settings(State(st): State<AppState>) -> impl IntoResponse {
    let live = st.settings().await;
    Json(settings_dto(&live))
}

/// Replace the settings row.
///
/// Takes the whole object rather than a patch: the clamps in `sanitize` are
/// relational — the floor must sit below strong, which must sit below
/// exceptional — so a field can only be validated against the rest of the set,
/// not on its own. The response is the settings as *stored*, which is how the
/// client learns what got clamped.
async fn put_settings(
    State(st): State<AppState>,
    Json(mut incoming): Json<Settings>,
) -> impl IntoResponse {
    // The company lists are read from disk, so anything sent for them is
    // ignored — but the stored fallback must survive, or a client that round
    // trips the settings object wipes it.
    let current = st.settings().await;
    incoming.greenhouse_boards = current.greenhouse_boards.clone();
    incoming.workday_sites = current.workday_sites.clone();
    // The resume is uploaded through its own endpoint (it arrives as a PDF and
    // is extracted server-side), so it is never overwritten from here.
    incoming.resume = current.resume.clone();
    incoming.resume_filename = current.resume_filename.clone();
    incoming.resume_updated_at = current.resume_updated_at;
    incoming.sanitize();

    if let Err(e) = db::save_settings(&st.pool, &incoming).await {
        tracing::warn!(%e, "settings save failed");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    // set_settings syncs the status view first, so the dashboard never shows a
    // source as live-and-failing in the instant it was switched off.
    st.set_settings(incoming.clone()).await;
    // years_experience lives in settings but feeds the profile, so a save has
    // to move it across or the ATS keeps scoring against the old number.
    st.rebuild_profile(&incoming).await;
    // The score means something different now, so the board is brought onto
    // the new scale rather than left holding two.
    if let Err(e) = crate::pipeline::rescore_all(&st).await {
        tracing::warn!(%e, "re-score after settings save failed");
    }
    st.notify_ui();
    Json(settings_dto(&incoming)).into_response()
}

#[derive(Deserialize, Debug, Default)]
pub struct DraftBody {
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub body: String,
}

/// Send the application, then report what happened.
///
/// A failed send deliberately leaves the card on the board rather than marking
/// it sent: SMTP being down should cost you a retry, not the job.
async fn send(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    Json(f): Json<DraftBody>,
) -> impl IntoResponse {
    let Ok(Some(c)) = db::get(&st.pool, id).await else {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "no such candidate"})))
            .into_response();
    };
    let _ = db::set_draft(&st.pool, id, &f.subject, &f.body).await;

    match crate::notify::send_application(&st.cfg, &c, &f.subject, &f.body).await {
        Ok(_) => {
            let _ = db::set_status(&st.pool, id, "sent").await;
            st.notify_ui();
            Json(serde_json::json!({"sent": true, "status": "sent"})).into_response()
        }
        Err(e) => {
            tracing::warn!(id, %e, "application send failed");
            st.notify_ui();
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "sent": false,
                    "status": c.status,
                    "error": e.to_string()
                })),
            )
                .into_response()
        }
    }
}

async fn dismiss(State(st): State<AppState>, Path(id): Path<i64>) -> impl IntoResponse {
    set_status(&st, id, "dismissed").await
}

async fn applied(State(st): State<AppState>, Path(id): Path<i64>) -> impl IntoResponse {
    set_status(&st, id, "applied").await
}

async fn set_status(st: &AppState, id: i64, status: &str) -> axum::response::Response {
    match db::set_status(&st.pool, id, status).await {
        Ok(_) => {
            st.notify_ui();
            Json(serde_json::json!({"id": id, "status": status})).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// Persist an edited draft without sending. The DM and external-apply cards have
/// nothing to submit to — their drafts are copied out by hand — so without this
/// the edit is lost on the next refresh.
async fn save_draft(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    Json(f): Json<DraftBody>,
) -> impl IntoResponse {
    match db::set_draft(&st.pool, id, &f.subject, &f.body).await {
        Ok(_) => Json(serde_json::json!({"saved": true})).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"saved": false, "error": e.to_string()})),
        )
            .into_response(),
    }
}

// ===================== assembly =====================

/// The radar window, from the query string if given and sane, else settings.
fn window_hours(q: &HashMap<String, String>, live: &Settings) -> i64 {
    q.get("hours")
        .and_then(|h| h.trim().parse::<i64>().ok())
        .filter(|h| (1..=24 * 30).contains(h))
        .unwrap_or(live.radar_hours)
}

fn filter_from(q: &HashMap<String, String>) -> db::RadarFilter {
    let get = |k: &str| q.get(k).map(|s| s.trim().to_string()).unwrap_or_default();
    // Multi-select facts arrive as one comma string, the same shape as tags.
    let list = |k: &str| -> Vec<String> {
        get(k)
            .split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_lowercase)
            .collect()
    };
    let num = |k: &str| get(k).parse::<i64>().ok().filter(|n| (0..=40).contains(n));

    db::RadarFilter {
        q: get("q"),
        source: get("source"),
        status: get("status"),
        tier: get("tier"),
        min_score: get("min").parse::<f64>().unwrap_or(0.0).clamp(0.0, 100.0),
        tags: get("tags")
            .split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .collect(),
        roles: list("roles"),
        levels: list("levels"),
        work_modes: list("modes"),
        regions: list("regions"),
        verdicts: list("verdicts"),
        // "I have N years" -> show me anything asking for at most N.
        years_max_wanted: num("yrs_have"),
        // "at least N years of seniority" -> anything whose ceiling reaches N.
        years_min_wanted: num("yrs_min"),
        sort: db::Sort::parse(&get("sort")),
    }
}

async fn radar_page(st: &AppState, hours: i64, q: &HashMap<String, String>) -> RadarPage {
    let window = hours * 3600;
    let filter = filter_from(q);
    let limit = q
        .get("limit")
        .and_then(|l| l.trim().parse::<i64>().ok())
        .unwrap_or(200)
        .clamp(1, 1000);

    let items = db::recent_filtered(&st.pool, window, &filter, limit)
        .await
        .unwrap_or_default();
    let (sources, statuses) = db::radar_facets(&st.pool, window).await.unwrap_or_default();
    let tags = db::tag_facets(&st.pool, window).await.unwrap_or_default();
    let total_in_window = db::recent_count(&st.pool, window).await.unwrap_or(0);

    RadarPage {
        items: items.iter().map(CardDto::from).collect(),
        sort: filter.sort.as_str().to_string(),
        window_hours: hours,
        total_in_window,
        // radar_facets returns the distinct values present in the window, with
        // no counts — a count per source would be another scan per facet, and
        // the source and status chips are a short fixed list the user already
        // knows. The tag chips DO carry counts, because there are dozens of
        // them and "which of these is worth clicking" is the actual question.
        sources: sources
            .into_iter()
            .map(|v| Facet {
                label: crate::web::source_label(&v).to_string(),
                value: v,
                count: None,
            })
            .collect(),
        statuses: statuses
            .into_iter()
            .map(|v| Facet {
                label: v.clone(),
                value: v,
                count: None,
            })
            .collect(),
        tags: tags
            .into_iter()
            .map(|(v, count)| Facet {
                label: v.clone(),
                value: v,
                count: Some(count),
            })
            .collect(),
        roles: fact_facets(st, "role", window).await,
        levels: fact_facets(st, "level", window).await,
        work_modes: fact_facets(st, "work_mode", window).await,
        regions: fact_facets(st, "region", window).await,
        verdicts: fact_facets(st, "verdict", window).await,
        gaps: db::common_gaps(&st.pool, window, 8)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|(v, count)| Facet { label: v.clone(), value: v, count: Some(count) })
            .collect(),
        // Only meaningful when something is actually going to read them. With
        // no model configured these rows stay NULL forever, and a caption
        // promising they'll sharpen would be a lie that never resolves.
        pending_enrichment: if st.enricher.is_llm() {
            db::unenriched_count(&st.pool).await.unwrap_or(0)
        } else {
            0
        },
    }
}

async fn fact_facets(st: &AppState, column: &str, window: i64) -> Vec<Facet> {
    db::fact_facets(&st.pool, column, window)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(v, count)| Facet {
            label: v.clone(),
            value: v,
            count: Some(count),
        })
        .collect()
}

async fn outbox_page(st: &AppState, live: &Settings) -> OutboxPage {
    let mut items = db::outbox(&st.pool, live.outbox_min, 100)
        .await
        .unwrap_or_default();
    let total = db::outbox_count(&st.pool, live.outbox_min)
        .await
        .unwrap_or(0);
    crate::draft::fill_missing(st, &mut items, live).await;
    OutboxPage {
        items: items.iter().map(CardDto::from).collect(),
        total,
        min_score: live.outbox_min,
    }
}

async fn status_dto(st: &AppState, hours: i64) -> StatusDto {
    let live = st.settings().await;
    let snap = st.status_snapshot().await;
    let rows_in_window = db::recent_count(&st.pool, hours * 3600).await.unwrap_or(0);
    let (level, message) = crate::status::diagnosis(&snap, live.score_floor, rows_in_window);

    StatusDto {
        level: match level {
            crate::status::Level::Ok => "ok",
            crate::status::Level::Info => "info",
            crate::status::Level::Warn => "warn",
            crate::status::Level::Error => "error",
        }
        .into(),
        message,
        sources: snap
            .sources
            .iter()
            .map(|(name, s)| SourceDto {
                name: name.clone(),
                label: crate::web::source_label(name).to_string(),
                enabled: s.enabled,
                running: s.running,
                last_run: s.last_run,
                next_run: s.next_run,
                last_error: s.last_error.clone(),
                notes: s
                    .notes
                    .iter()
                    .map(|n| NoteDto {
                        text: n.text.clone(),
                        ok: n.ok,
                    })
                    .collect(),
                fetched: s.fetched,
                new_posts: s.new_posts,
                stored: s.stored,
                below_floor: s.below_floor,
                wrong_location: s.wrong_location,
                wrong_stack: s.wrong_stack,
                not_hiring: s.not_hiring,
                total_fetched: s.total_fetched,
                total_stored: s.total_stored,
                best_score: round1(s.best_score),
            })
            .collect(),
        rows_in_window,
        window_hours: hours,
        outbox_count: db::outbox_count(&st.pool, live.outbox_min)
            .await
            .unwrap_or(0),
    }
}

fn settings_dto(live: &Settings) -> SettingsDto {
    let profile = crate::ats::Profile::from_resume(&live.resume, live.years_experience);
    use crate::sources::common::companies;
    let lists = [
        ("greenhouse", &live.greenhouse_boards),
        ("lever", &live.lever_boards),
        ("workday", &live.workday_sites),
    ]
    .into_iter()
    .map(|(kind, stored)| {
        let list = companies::load(kind);
        CompanyListDto {
            kind: kind.to_string(),
            path: list.path.display().to_string(),
            entries: companies::resolve(&list, stored),
            read_only: true,
            note: list.note(),
        }
    })
    .collect();

    SettingsDto {
        profile,
        company_lists: lists,
        has_resume: live.has_resume(),
        resume_chars: live.resume.len(),
        settings: live.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate() -> Candidate {
        Candidate {
            id: 1,
            urn: "u".into(),
            source: "linkedin_voyager".into(),
            url: "https://example.com/p".into(),
            title: "Backend Engineer".into(),
            company: "Acme".into(),
            location: Some("Bengaluru, India".into()),
            body: "word ".repeat(300),
            score: 78.349,
            priority: 78.349,
            tier: "strong".into(),
            status: "queued".into(),
            detected_at: crate::model::now() - 600,
            posted_at: Some(crate::model::now() - 600),
            expires_at: 0,
            settle_until: 0,
            apply_kind: "email".into(),
            apply_target: Some("a@b.com".into()),
            draft_subject: None,
            draft_body: None,
            notified_at: None,
            match_terms: Some("go, kafka".into()),
            tags: Some(",Go,Kafka,".into()),
            role: Some("backend".into()),
            level: Some("senior".into()),
            years_min: Some(5),
            years_max: None,
            work_mode: Some("hybrid".into()),
            employment: Some("full-time".into()),
            region: Some("india".into()),
            verdict: Some("apply".into()),
            reason: Some("Meets what it asks for.".into()),
            missing: None,
            dimensions: None,
            enriched_at: None,
        }
    }

    #[test]
    fn the_year_range_is_formatted_server_side() {
        // Two surfaces formatting "5+ yrs" differently is exactly the drift the
        // DTO exists to prevent.
        assert_eq!(CardDto::from(&candidate()).years.as_deref(), Some("5+ yrs"));
    }

    #[test]
    fn a_card_says_whether_the_model_has_read_it_yet() {
        // The facts sharpen as the queue drains; a board that changes under you
        // deserves to say so.
        assert!(!CardDto::from(&candidate()).enriched);
        let mut c = candidate();
        c.enriched_at = Some(1);
        assert!(CardDto::from(&c).enriched);
    }

    #[test]
    fn fact_filters_parse_as_lowercase_multi_select() {
        let q = HashMap::from([
            ("roles".to_string(), "Backend, SRE".to_string()),
            ("yrs_have".to_string(), "5".to_string()),
        ]);
        let f = filter_from(&q);
        assert_eq!(f.roles, vec!["backend", "sre"]);
        assert_eq!(f.years_max_wanted, Some(5));
    }

    #[test]
    fn region_filters_are_multi_select_like_the_rest() {
        // "India or the Gulf" is the question; the intersection of two regions
        // is empty by definition.
        let q = HashMap::from([("regions".to_string(), "India, Gulf".to_string())]);
        assert_eq!(filter_from(&q).regions, vec!["india", "gulf"]);
    }

    #[test]
    fn a_nonsense_year_filter_is_ignored_rather_than_emptying_the_board() {
        let q = HashMap::from([("yrs_have".to_string(), "banana".to_string())]);
        assert_eq!(filter_from(&q).years_max_wanted, None);
    }

    #[test]
    fn a_card_never_carries_the_whole_body() {
        // A hundred cards of full job description is megabytes to paint a list
        // nobody has scrolled yet.
        let dto = CardDto::from(&candidate());
        assert!(dto.excerpt.len() < 500, "{}", dto.excerpt.len());
        assert!(dto.excerpt.ends_with('…'));
    }

    #[test]
    fn scores_are_rounded_once_at_the_boundary() {
        // Otherwise two views show 78.3 and 78.35 for the same card.
        assert_eq!(CardDto::from(&candidate()).score, 78.3);
    }

    #[test]
    fn tags_arrive_as_a_list_not_as_the_storage_encoding() {
        assert_eq!(CardDto::from(&candidate()).tags, vec!["Go", "Kafka"]);
    }

    #[test]
    fn the_source_label_is_decided_server_side() {
        // Two frontends must not get to disagree about what a source is called.
        assert_eq!(CardDto::from(&candidate()).source_label, "li·posts");
    }

    #[test]
    fn a_short_body_is_not_truncated_or_ellipsised() {
        let mut c = candidate();
        c.body = "Short and complete.".into();
        assert_eq!(CardDto::from(&c).excerpt, "Short and complete.");
    }

    #[test]
    fn an_empty_why_is_absent_rather_than_blank() {
        let mut c = candidate();
        c.match_terms = Some("   ".into());
        assert!(CardDto::from(&c).why.is_none());
    }

    #[test]
    fn the_window_comes_from_the_query_only_when_it_is_sane() {
        let live: Settings = serde_json::from_str("{}").unwrap();
        let q = |v: &str| HashMap::from([("hours".to_string(), v.to_string())]);
        assert_eq!(window_hours(&q("6"), &live), 6);
        // Nonsense must fall back rather than produce an empty board.
        assert_eq!(window_hours(&q("0"), &live), live.radar_hours);
        assert_eq!(window_hours(&q("banana"), &live), live.radar_hours);
        assert_eq!(window_hours(&q("99999"), &live), live.radar_hours);
    }

    #[test]
    fn an_unknown_sort_falls_back_rather_than_erroring() {
        // A stale bookmark with ?sort=priority should show the board, not a 400.
        assert_eq!(db::Sort::parse("priority"), db::Sort::Newest);
        assert_eq!(db::Sort::parse(""), db::Sort::Newest);
        assert_eq!(db::Sort::parse("SCORE"), db::Sort::Score);
        assert_eq!(db::Sort::parse(" oldest "), db::Sort::Oldest);
    }

    #[test]
    fn the_sort_round_trips_through_its_own_name() {
        for s in [db::Sort::Newest, db::Sort::Oldest, db::Sort::Score] {
            assert_eq!(db::Sort::parse(s.as_str()), s);
        }
    }

    #[test]
    fn tag_filters_parse_from_one_comma_string() {
        let q = HashMap::from([("tags".to_string(), "Go, Rust ,".to_string())]);
        assert_eq!(filter_from(&q).tags, vec!["Go", "Rust"]);
    }

    #[test]
    fn settings_expose_the_company_lists_as_read_only() {
        let live: Settings = serde_json::from_str("{}").unwrap();
        let dto = settings_dto(&live);
        assert_eq!(dto.company_lists.len(), 3);
        assert!(dto.company_lists.iter().all(|l| l.read_only));
        assert!(dto.company_lists.iter().all(|l| !l.note.is_empty()));
    }

    /// A round trip must not be able to erase the fallback lists or the resume.
    #[test]
    fn settings_flatten_without_swallowing_the_extras() {
        let live: Settings = serde_json::from_str("{}").unwrap();
        let v = serde_json::to_value(settings_dto(&live)).unwrap();
        // Flattened, so settings fields sit at the top level next to the extras.
        assert!(v.get("score_floor").is_some(), "{v}");
        assert!(v.get("company_lists").is_some(), "{v}");
        assert!(v.get("has_resume").is_some(), "{v}");
    }
}
