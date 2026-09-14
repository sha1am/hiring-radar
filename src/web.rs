use crate::db;
use crate::model::Candidate;
use crate::notify;
use crate::settings::{parse_list, Query as LiQuery, Settings};
use crate::state::AppState;
use crate::status::{diagnosis, since, until, Level};
use axum::extract::{DefaultBodyLimit, Multipart, Path, Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Form, Router};
use futures::stream::StreamExt;
use serde::Deserialize;
use std::convert::Infallible;
use tokio_stream::wrappers::BroadcastStream;

// NOTE: every HTML literal in this file uses r##"..."## rather than r#"..."#.
// htmx attributes are full of `hx-target="#queue"`, and the `"#` in that closes
// an r#"..."# literal — which is exactly how this file failed to compile once.
// Keep the doubled hashes even where a given literal doesn't need them.

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(page))
        .route("/queue", get(queue))
        .route("/radar", get(radar))
        .route("/status", get(status_fragment))
        .route("/events", get(events))
        .route("/candidate/:id/send", post(send))
        .route("/candidate/:id/dismiss", post(dismiss))
        .route("/candidate/:id/applied", post(applied))
        .route("/candidate/:id/draft", post(save_draft))
        .route("/settings", get(settings_page).post(settings_save))
        .route(
            "/settings/resume",
            // A PDF resume comfortably exceeds axum's 2MB default.
            post(resume_upload).layer(DefaultBodyLimit::max(20 * 1024 * 1024)),
        )
        .route("/settings/resume/clear", post(resume_clear))
        .route("/assets/htmx.min.js", get(htmx_asset))
        .with_state(state)
}

async fn page(State(st): State<AppState>) -> impl IntoResponse {
    let hours = st.settings().await.radar_hours;
    let stat = render_status(&st, hours).await;
    let q = render_queue(&st).await;
    let r = render_radar(&st, hours, &RadarQuery::default()).await;
    let h = render_history(&st).await;
    Html(shell(&stat, &q, &r, &h))
}

/// htmx is served from the binary rather than a CDN.
///
/// Every interactive part of this dashboard is htmx — the filters, saving
/// settings, dismiss and apply. Loading it from unpkg means that when you are
/// offline, or the CDN has a bad day, a self-hosted tool running in a container
/// on your own machine silently loses all of its buttons. 48KB embedded is a
/// cheap way to never think about that again.
async fn htmx_asset() -> impl IntoResponse {
    (
        [
            (axum::http::header::CONTENT_TYPE, "application/javascript; charset=utf-8"),
            (axum::http::header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
        ],
        include_str!("../assets/htmx.min.js"),
    )
}

async fn status_fragment(State(st): State<AppState>) -> impl IntoResponse {
    let hours = st.settings().await.radar_hours;
    Html(render_status(&st, hours).await)
}

async fn queue(State(st): State<AppState>) -> impl IntoResponse {
    Html(render_queue(&st).await)
}

#[derive(Default)]
struct RadarQuery {
    hours: Option<i64>,
    q: String,
    source: String,
    status: String,
    tier: String,
    min: String,
}

impl RadarQuery {
    /// Built from a map rather than derived, because htmx can legitimately send
    /// the same key twice — a control with its own query string plus an
    /// inherited hx-include that also carries it. A derived extractor rejects
    /// that with a 400, which reaches the page as a control that simply stops
    /// working. Last value wins.
    fn from_map(m: &std::collections::HashMap<String, String>) -> Self {
        let get = |k: &str| m.get(k).cloned().unwrap_or_default();
        Self {
            hours: m.get("hours").and_then(|v| v.parse::<i64>().ok()),
            q: get("q"),
            source: get("source"),
            status: get("status"),
            tier: get("tier"),
            min: get("min"),
        }
    }
}

impl RadarQuery {
    fn filter(&self) -> db::RadarFilter {
        db::RadarFilter {
            q: self.q.trim().to_lowercase(),
            source: self.source.trim().to_string(),
            status: self.status.trim().to_string(),
            tier: self.tier.trim().to_string(),
            min_score: self.min.trim().parse::<f64>().unwrap_or(0.0).clamp(0.0, 100.0),
        }
    }

    /// The current filter state as a query string, so the window buttons and
    /// any other control can carry it rather than silently resetting it.
    fn qs(&self, hours: i64) -> String {
        let mut p = vec![format!("hours={hours}")];
        for (k, v) in [
            ("q", self.q.as_str()),
            ("source", self.source.as_str()),
            ("status", self.status.as_str()),
            ("tier", self.tier.as_str()),
            ("min", self.min.as_str()),
        ] {
            if !v.trim().is_empty() {
                p.push(format!("{k}={}", urlencode(v.trim())));
            }
        }
        p.join("&")
    }
}

/// Minimal percent-encoding for values going back into an hx-get URL.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

async fn radar(
    State(st): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let q = RadarQuery::from_map(&params);
    let default_hours = st.settings().await.radar_hours;
    let hours = q.hours.unwrap_or(default_hours).clamp(1, 24 * 30);
    Html(render_radar(&st, hours, &q).await)
}

/// SSE stream: every queue change pushes a tick; the browser reloads the lists.
async fn events(
    State(st): State<AppState>,
) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    let rx = st.events.subscribe();
    let stream = BroadcastStream::new(rx).map(|_| Ok(Event::default().data("tick")));
    Sse::new(stream).keep_alive(KeepAlive::default())
}

#[derive(Deserialize)]
struct DraftForm {
    #[serde(default)]
    subject: String,
    #[serde(default)]
    body: String,
}

async fn send(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    Form(f): Form<DraftForm>,
) -> impl IntoResponse {
    if let Ok(Some(c)) = db::get(&st.pool, id).await {
        let _ = db::set_draft(&st.pool, id, &f.subject, &f.body).await;
        match notify::send_application(&st.cfg, &c, &f.subject, &f.body).await {
            Ok(_) => {
                let _ = db::set_status(&st.pool, id, "sent").await;
            }
            Err(e) => {
                tracing::warn!(id, %e, "application send failed");
                // Leave it on the board so you can retry or send manually.
            }
        }
    }
    st.notify_ui();
    Html(render_queue(&st).await)
}

async fn dismiss(State(st): State<AppState>, Path(id): Path<i64>) -> impl IntoResponse {
    let _ = db::set_status(&st.pool, id, "dismissed").await;
    st.notify_ui();
    Html(render_queue(&st).await)
}

async fn applied(State(st): State<AppState>, Path(id): Path<i64>) -> impl IntoResponse {
    let _ = db::set_status(&st.pool, id, "applied").await;
    st.notify_ui();
    Html(render_queue(&st).await)
}

/// Persist an edited draft without sending. The DM and external-apply cards have
/// no submit button — their drafts are copied out by hand — so without this the
/// edit is lost on the next refresh.
async fn save_draft(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    Form(f): Form<DraftForm>,
) -> impl IntoResponse {
    match db::set_draft(&st.pool, id, &f.subject, &f.body).await {
        Ok(_) => Html(r##"<span class="text-xs text-emerald-400">saved</span>"##.to_string()),
        Err(e) => {
            tracing::warn!(id, %e, "draft save failed");
            Html(r##"<span class="text-xs text-rose-400">save failed</span>"##.to_string())
        }
    }
}

// ----- rendering -----

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Post URLs come from third-party payloads. Anything that isn't plainly http(s)
/// is not going in an href — `javascript:` in a link the user is invited to click
/// is the whole attack.
fn safe_url(u: &str) -> String {
    let t = u.trim();
    if t.starts_with("https://") || t.starts_with("http://") {
        esc(t)
    } else {
        "#".to_string()
    }
}

fn tier_badge(tier: &str) -> &'static str {
    match tier {
        "exceptional" => "bg-rose-500/15 text-rose-300 ring-rose-500/30",
        "strong" => "bg-amber-500/15 text-amber-300 ring-amber-500/30",
        _ => "bg-slate-500/15 text-slate-300 ring-slate-500/30",
    }
}

fn status_chip(status: &str) -> (&'static str, &'static str) {
    match status {
        "notified" => ("text-sky-300 bg-sky-500/10 ring-sky-500/30", "alerted"),
        "sent" => ("text-emerald-300 bg-emerald-500/10 ring-emerald-500/30", "sent"),
        "applied" => ("text-emerald-300 bg-emerald-500/10 ring-emerald-500/30", "applied"),
        "dismissed" => ("text-slate-500 bg-slate-500/10 ring-slate-600/30", "dismissed"),
        "digested" => ("text-slate-400 bg-slate-500/10 ring-slate-600/30", "digest"),
        "backfilled" => ("text-slate-400 bg-slate-500/10 ring-slate-600/30", "history"),
        "expired" => ("text-slate-600 bg-slate-500/5 ring-slate-700/30", "expired"),
        _ => ("text-violet-300 bg-violet-500/10 ring-violet-500/30", "pooled"),
    }
}

fn source_label(source: &str) -> &str {
    match source {
        "greenhouse" => "greenhouse",
        "linkedin_guest" => "li·jobs",
        "linkedin_voyager" => "li·posts",
        other => other,
    }
}

/// Why this post scored. With a resume loaded these are the resume terms the
/// post actually hit — the difference between "94" and "94, because it wants
/// exactly the Kafka and Kubernetes work you've been doing".
fn why_matched(c: &Candidate) -> String {
    match c.match_terms.as_deref().filter(|t| !t.trim().is_empty()) {
        Some(t) => format!(
            r##"<p class="mt-2 text-xs text-slate-500">Matched your resume on <span class="text-slate-400">{}</span></p>"##,
            esc(t)
        ),
        None => String::new(),
    }
}

fn card(c: &Candidate) -> String {
    let draft_subject = esc(c.draft_subject.as_deref().unwrap_or(""));
    let draft_body = esc(c.draft_body.as_deref().unwrap_or(""));
    let loc = esc(&c.location.clone().unwrap_or_default());

    // Action row depends on how you actually reach this poster.
    let actions = match c.apply_kind.as_str() {
        "email" => format!(
            r##"<button type="submit"
                 class="rounded-md bg-emerald-500/90 hover:bg-emerald-400 px-3 py-1.5 text-sm font-medium text-slate-900">
                 Send email &rarr; {to}</button>"##,
            to = esc(&c.apply_target.clone().unwrap_or_default())
        ),
        "dm" => format!(
            r##"<button type="button" onclick="copyBody({id})"
                 class="rounded-md bg-sky-500/90 hover:bg-sky-400 px-3 py-1.5 text-sm font-medium text-slate-900">Copy draft</button>
               <a target="_blank" rel="noopener noreferrer" href="{url}"
                 class="rounded-md bg-slate-700 hover:bg-slate-600 px-3 py-1.5 text-sm">Open profile &nearr;</a>"##,
            id = c.id,
            url = safe_url(&c.url)
        ),
        _ => format!(
            r##"<a target="_blank" rel="noopener noreferrer" href="{url}"
                 class="rounded-md bg-sky-500/90 hover:bg-sky-400 px-3 py-1.5 text-sm font-medium text-slate-900">Open apply page &nearr;</a>"##,
            url = safe_url(&c.url)
        ),
    };

    let subject_field = if c.apply_kind == "email" {
        format!(
            r##"<input name="subject" value="{subj}"
                 class="w-full rounded-md bg-slate-900/60 border border-slate-700 px-3 py-1.5 text-sm mb-2"/>"##,
            subj = draft_subject
        )
    } else {
        String::new()
    };

    format!(
        r##"<article id="card-{id}" class="rounded-xl bg-slate-800/60 ring-1 ring-slate-700/60 p-4">
  <div class="flex items-start justify-between gap-3">
    <div class="min-w-0">
      <h3 class="font-semibold text-slate-100 truncate">{title}</h3>
      <p class="text-sm text-slate-400 truncate">{company} &middot; {loc} &middot; {age}</p>
    </div>
    <div class="flex items-center gap-2 shrink-0">
      <span class="text-xs px-2 py-0.5 rounded-full ring-1 {badge}">{tier}</span>
      <span class="text-sm font-mono text-slate-300">{score:.0}</span>
    </div>
  </div>
  {why}
  <form hx-post="/candidate/{id}/send" hx-target="#queue" hx-swap="outerHTML" class="mt-3">
    {subject_field}
    <textarea id="body-{id}" name="body" rows="6"
      class="w-full rounded-md bg-slate-900/60 border border-slate-700 px-3 py-2 text-sm font-mono leading-relaxed">{body}</textarea>
    <div class="mt-3 flex flex-wrap items-center gap-2">
      {actions}
      <button type="button" hx-post="/candidate/{id}/draft" hx-include="#card-{id} textarea, #card-{id} input[name=subject]"
        hx-target="#saved-{id}" hx-swap="innerHTML"
        class="rounded-md bg-slate-700 hover:bg-slate-600 px-3 py-1.5 text-sm">Save draft</button>
      <span id="saved-{id}"></span>
      <span class="flex-1"></span>
      <button type="button" hx-post="/candidate/{id}/applied" hx-target="#queue" hx-swap="outerHTML"
        class="rounded-md bg-slate-700 hover:bg-slate-600 px-3 py-1.5 text-sm">Mark applied</button>
      <button type="button" hx-post="/candidate/{id}/dismiss" hx-target="#queue" hx-swap="outerHTML"
        class="rounded-md text-slate-400 hover:text-slate-200 px-3 py-1.5 text-sm">Dismiss</button>
    </div>
  </form>
</article>"##,
        id = c.id,
        title = esc(&c.title),
        company = esc(&c.company),
        loc = loc,
        age = esc(&c.age_str()),
        why = why_matched(c),
        badge = tier_badge(&c.tier),
        tier = c.tier,
        score = c.score,
        body = draft_body,
        subject_field = subject_field,
        actions = actions,
    )
}

async fn render_queue(st: &AppState) -> String {
    let items = db::active(&st.pool).await.unwrap_or_default();
    let cap = st.settings().await.per_hour_cap;
    let used = db::budget_used(&st.pool).await.unwrap_or(0);

    let cards = if items.is_empty() {
        r##"<p class="text-slate-500 text-sm py-6 text-center">Nothing waiting on you. New matches land here the moment they fire.</p>"##.to_string()
    } else {
        items.iter().map(card).collect::<Vec<_>>().join("\n")
    };

    format!(
        r##"<section id="queue" class="space-y-3">
  <div class="flex items-center justify-between">
    <h2 class="text-sm uppercase tracking-wide text-slate-400">Awaiting you</h2>
    <span class="text-xs text-slate-500">{used}/{cap} alerts sent this hour</span>
  </div>
  {cards}
</section>"##,
        used = used,
        cap = cap,
        cards = cards
    )
}

fn radar_row(c: &Candidate) -> String {
    let (chip_class, chip) = status_chip(&c.status);
    format!(
        r##"<li class="flex items-center gap-3 py-2 border-b border-slate-800/60 last:border-0">
  <span class="w-9 shrink-0 text-right font-mono text-sm {score_tone}">{score:.0}</span>
  <span class="w-1.5 h-1.5 rounded-full shrink-0 {dot}"></span>
  <a href="{url}" target="_blank" rel="noopener noreferrer" class="min-w-0 flex-1 group">
    <span class="block truncate text-slate-200 group-hover:text-white">{title}</span>
    <span class="block truncate text-xs text-slate-500">{company}{loc} &middot; {source}{why}</span>
  </a>
  <span class="shrink-0 text-xs text-slate-500 tabular-nums">{age}</span>
  <span class="shrink-0 text-[10px] uppercase px-1.5 py-0.5 rounded ring-1 {chip_class}">{chip}</span>
</li>"##,
        score = c.score,
        score_tone = if c.score >= 85.0 {
            "text-rose-300"
        } else if c.score >= 70.0 {
            "text-amber-300"
        } else {
            "text-slate-400"
        },
        dot = match c.tier.as_str() {
            "exceptional" => "bg-rose-400",
            "strong" => "bg-amber-400",
            _ => "bg-slate-600",
        },
        url = safe_url(&c.url),
        title = esc(&c.title),
        company = esc(&c.company),
        loc = c
            .location
            .as_deref()
            .filter(|l| !l.trim().is_empty())
            .map(|l| format!(" &middot; {}", esc(l)))
            .unwrap_or_default(),
        source = esc(source_label(&c.source)),
        why = c
            .match_terms
            .as_deref()
            .filter(|t| !t.trim().is_empty())
            .map(|t| format!(" &middot; <span class=\"text-slate-600\">{}</span>", esc(t)))
            .unwrap_or_default(),
        age = esc(&c.age_str()),
        chip_class = chip_class,
        chip = chip,
    )
}

fn window_button(hours: i64, active: i64, label: &str, q: &RadarQuery) -> String {
    let cls = if hours == active {
        "bg-slate-700 text-slate-100"
    } else {
        "text-slate-500 hover:text-slate-300"
    };
    // Carries the current filters, so changing the window narrows the same
    // search rather than silently clearing it.
    format!(
        r##"<button hx-get="/radar?{qs}" hx-target="#radar" hx-swap="outerHTML"
      class="px-2 py-0.5 rounded text-xs {cls}">{label}</button>"##,
        qs = q.qs(hours),
        cls = cls,
        label = label
    )
}

fn opt_tag(value: &str, label: &str, selected: &str) -> String {
    format!(
        r##"<option value="{v}" {sel}>{l}</option>"##,
        v = esc(value),
        l = esc(label),
        sel = if value == selected { "selected" } else { "" }
    )
}

/// The filter bar.
///
/// Every control posts the whole bar (hx-include), so the filters compose
/// instead of each one resetting the others. Filtering runs in SQL over the
/// whole window — filtering the rendered list would only search the newest few
/// hundred rows and miss matches further back.
fn filter_bar(q: &RadarQuery, hours: i64, sources: &[String], statuses: &[String]) -> String {
    let src_opts: String = std::iter::once(opt_tag("", "any source", &q.source))
        .chain(
            sources
                .iter()
                .map(|s| opt_tag(s, source_label(s), &q.source)),
        )
        .collect();
    let status_opts: String = std::iter::once(opt_tag("", "any status", &q.status))
        .chain(statuses.iter().map(|s| {
            let (_, label) = status_chip(s);
            opt_tag(s, label, &q.status)
        }))
        .collect();
    let tier_opts: String = [("", "any tier"), ("exceptional", "exceptional"), ("strong", "strong"), ("marginal", "marginal")]
        .iter()
        .map(|(v, l)| opt_tag(v, l, &q.tier))
        .collect();
    let min_opts: String = [("", "any score"), ("50", "50+"), ("65", "65+"), ("75", "75+"), ("85", "85+")]
        .iter()
        .map(|(v, l)| opt_tag(v, l, &q.min))
        .collect();

    let sel = "rounded-md bg-slate-900/60 border border-slate-700 px-2 py-1 text-xs text-slate-300";
    let clear = if q.filter().is_active() {
        format!(
            // hx-include is inherited from the wrapper, so without unsetting it
            // "clear" would helpfully re-send everything it is meant to clear.
            r##"<button hx-get="/radar?hours={hours}" hx-target="#radar" hx-swap="outerHTML"
        hx-include="unset"
        class="text-xs text-slate-500 hover:text-slate-300 underline">clear</button>"##,
            hours = hours
        )
    } else {
        String::new()
    };

    format!(
        r##"<div id="radar-filters" class="mt-2 flex flex-wrap items-center gap-2"
     hx-get="/radar" hx-target="#radar" hx-swap="outerHTML"
     hx-include="#radar-filters [name]" hx-trigger="change">
  <input type="hidden" name="hours" value="{hours}"/>
  <input type="search" name="q" value="{q_val}" placeholder="search title, company, place, matched terms"
    class="flex-1 min-w-[12rem] rounded-md bg-slate-900/60 border border-slate-700 px-2 py-1 text-xs"
    hx-get="/radar" hx-target="#radar" hx-swap="outerHTML"
    hx-include="#radar-filters [name]" hx-trigger="keyup changed delay:350ms, search, change"/>
  <select name="min" class="{sel}">{min_opts}</select>
  <select name="tier" class="{sel}">{tier_opts}</select>
  <select name="source" class="{sel}">{src_opts}</select>
  <select name="status" class="{sel}">{status_opts}</select>
  {clear}
</div>"##,
        hours = hours,
        q_val = esc(&q.q),
        sel = sel,
        min_opts = min_opts,
        tier_opts = tier_opts,
        src_opts = src_opts,
        status_opts = status_opts,
        clear = clear
    )
}

/// Everything seen in the trailing window, not just what fired. This is the
/// "what's out there" half of the dashboard — history included, so the board is
/// useful on the very first run instead of empty until something new posts.
async fn render_radar(st: &AppState, hours: i64, q: &RadarQuery) -> String {
    let window = hours * 3600;
    let filter = q.filter();
    let mut items = db::recent_filtered(&st.pool, window, &filter, 300)
        .await
        .unwrap_or_default();
    let total = db::recent_count(&st.pool, window).await.unwrap_or(0);
    let (sources, statuses) = db::radar_facets(&st.pool, window).await.unwrap_or_default();

    // Best match first — the list is for scanning, not for chronology.
    items.sort_by(|a, b| {
        b.live_priority()
            .partial_cmp(&a.live_priority())
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let rows = if items.is_empty() && filter.is_active() {
        // Distinguish "your filter matched nothing" from "nothing has arrived",
        // which otherwise look identical and send you debugging the wrong thing.
        format!(
            r##"<p class="text-slate-500 text-sm py-6 text-center">No match among the {total} in this window. <button hx-get="/radar?hours={hours}" hx-target="#radar" hx-swap="outerHTML" class="underline hover:text-slate-300">clear the filters</button>.</p>"##,
            total = total,
            hours = hours
        )
    } else if items.is_empty() {
        r##"<p class="text-slate-500 text-sm py-6 text-center">Nothing in this window yet. Widen it, or give the first crawl a minute.</p>"##.to_string()
    } else {
        format!(
            r##"<ul class="mt-1">{}</ul>"##,
            items.iter().map(radar_row).collect::<Vec<_>>().join("")
        )
    };

    let shown = if filter.is_active() {
        format!(", {} shown", items.len())
    } else if total > items.len() as i64 {
        format!(" (showing top {})", items.len())
    } else {
        String::new()
    };

    format!(
        r##"<section id="radar" class="mt-8">
  <div class="flex items-center justify-between gap-2">
    <h2 class="text-sm uppercase tracking-wide text-slate-400">
      Radar <span class="text-slate-600 normal-case">&middot; {total} in last {hours}h{shown}</span>
    </h2>
    <div class="flex items-center gap-1 shrink-0">{b6}{b24}{b72}{b168}</div>
  </div>
  {filters}
  {rows}
</section>"##,
        total = total,
        hours = hours,
        shown = shown,
        b6 = window_button(6, hours, "6h", q),
        b24 = window_button(24, hours, "24h", q),
        b72 = window_button(72, hours, "3d", q),
        b168 = window_button(168, hours, "7d", q),
        filters = filter_bar(q, hours, &sources, &statuses),
        rows = rows
    )
}

async fn render_history(st: &AppState) -> String {
    let items = db::history(&st.pool, 12).await.unwrap_or_default();
    if items.is_empty() {
        return String::new();
    }
    let rows = items
        .iter()
        .map(|c| {
            let color = match c.status.as_str() {
                "sent" | "applied" => "text-emerald-400",
                _ => "text-slate-500",
            };
            format!(
                r##"<li class="flex items-center justify-between py-1.5 border-b border-slate-800/60">
                    <span class="truncate text-slate-300">{title} &middot; <span class="text-slate-500">{company}</span></span>
                    <span class="{color} text-xs uppercase ml-3 shrink-0">{status}</span>
                   </li>"##,
                title = esc(&c.title),
                company = esc(&c.company),
                status = c.status,
                color = color
            )
        })
        .collect::<Vec<_>>()
        .join("");
    format!(
        r##"<section id="history" class="mt-8">
  <h2 class="text-sm uppercase tracking-wide text-slate-400 mb-2">Acted on</h2>
  <ul class="text-sm">{rows}</ul>
</section>"##,
        rows = rows
    )
}

fn shell(status: &str, queue: &str, radar: &str, history: &str) -> String {
    format!(
        r##"<!doctype html>
<html lang="en" class="dark">
<head>
  <meta charset="utf-8"/>
  <meta name="viewport" content="width=device-width, initial-scale=1"/>
  <title>Hiring Radar</title>
  <script src="https://cdn.tailwindcss.com"></script>
  <script src="/assets/htmx.min.js"></script>
</head>
<body class="bg-slate-950 text-slate-200 min-h-screen">
  <div class="max-w-3xl mx-auto px-4 py-8">
    <header class="flex items-center justify-between mb-6">
      <div class="flex items-center gap-2">
        <span class="text-xl">&#128225;</span>
        <h1 class="text-lg font-semibold">Hiring Radar</h1>
        <span id="live" class="ml-2 inline-flex items-center gap-1 text-xs text-emerald-400">
          <span class="w-1.5 h-1.5 rounded-full bg-emerald-400 animate-pulse"></span>live</span>
      </div>
      <div class="flex items-center gap-3">
        <button onclick="reloadAll()" class="text-xs text-slate-400 hover:text-slate-200">refresh</button>
        <a href="/settings" class="text-xs text-slate-400 hover:text-slate-200">settings</a>
      </div>
    </header>
    {status}
    {queue}
    {radar}
    {history}
  </div>

  <script>
    function reloadAll() {{
      htmx.ajax('GET', '/status', {{target:'#status', swap:'outerHTML'}});
      htmx.ajax('GET', '/queue', {{target:'#queue', swap:'outerHTML'}});
      // Rebuild the current filter state from the bar so a live update does
      // not silently reset what you were looking at.
      const bar = document.getElementById('radar-filters');
      let qs = '';
      if (bar) {{
        const p = new URLSearchParams();
        bar.querySelectorAll('[name]').forEach(el => {{ if (el.value) p.set(el.name, el.value); }});
        qs = p.toString();
      }}
      htmx.ajax('GET', '/radar?' + qs, {{target:'#radar', swap:'outerHTML'}});
      htmx.ajax('GET', '/', {{target:'#history', swap:'none'}});
    }}
    function copyBody(id) {{
      const el = document.getElementById('body-' + id);
      navigator.clipboard.writeText(el.value);
    }}
    // Live updates: the server pings on every queue change; we refetch the lists.
    // The countdown and per-source state move even when nothing fires, so the
    // status panel refreshes on its own rather than only on a queue change.
    setInterval(() => htmx.ajax('GET', '/status', {{target:'#status', swap:'outerHTML'}}), 10000);
    const es = new EventSource('/events');
    es.onmessage = () => reloadAll();
    es.onerror = () => {{ document.getElementById('live').style.opacity = 0.4; }};
  </script>
</body>
</html>"##,
        status = status,
        queue = queue,
        radar = radar,
        history = history
    )
}


// ===================== settings =====================

/// Every field arrives as a string because that is what a form posts, and every
/// one is optional because unchecked checkboxes send nothing at all. Parsing is
/// deliberately forgiving: a field that won't parse keeps its current value
/// rather than failing the whole save or silently becoming zero.
#[derive(Deserialize, Default)]
struct SettingsForm {
    titles: Option<String>,
    keywords: Option<String>,
    locations: Option<String>,
    remote_ok: Option<String>,
    location_policy: Option<String>,
    allow_unknown_location: Option<String>,
    seniority: Option<String>,
    dealbreakers: Option<String>,
    min_salary: Option<String>,

    per_hour_cap: Option<String>,
    per_poster_cap: Option<String>,
    score_floor: Option<String>,
    strong_min: Option<String>,
    exceptional_min: Option<String>,
    settle_strong_mins: Option<String>,
    candidate_ttl_hours: Option<String>,
    adaptive_threshold: Option<String>,
    adaptive_start: Option<String>,
    adaptive_end: Option<String>,

    mode: Option<String>,
    greenhouse_enabled: Option<String>,
    greenhouse_boards: Option<String>,
    linkedin_guest_enabled: Option<String>,
    linkedin_queries: Option<String>,
    voyager_enabled: Option<String>,
    voyager_queries: Option<String>,
    voyager_query_id: Option<String>,
    lookback_hours: Option<String>,
    voyager_max_pages: Option<String>,
    voyager_poll_pages: Option<String>,

    radar_hours: Option<String>,
    resume_weight: Option<String>,
}

fn num<T: std::str::FromStr>(v: &Option<String>, current: T) -> T {
    v.as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<T>().ok())
        .unwrap_or(current)
}

fn checked(v: &Option<String>) -> bool {
    v.is_some()
}

/// Renders settings with the resume signals the matcher actually derived,
/// which needs both the settings snapshot and the matcher.
async fn render_settings(st: &AppState, note: Option<Result<&str, &str>>) -> String {
    let s = st.settings().await;
    let m = st.matcher().await;
    let derived = m.resume.as_ref().map(|p| p.top_terms(14)).unwrap_or_default();
    settings_shell(&s, note, &derived)
}

async fn settings_page(State(st): State<AppState>) -> impl IntoResponse {
    Html(render_settings(&st, None).await)
}

/// 0..100 in the form, 0..1 in storage — a percentage is what a person can
/// reason about when balancing "my resume" against "my keyword list".
fn weight_slider(s: &Settings) -> String {
    if s.resume.trim().is_empty() {
        return String::new();
    }
    format!(
        r##"<label class="block pt-1">
  <span class="block text-sm text-slate-300">Resume weight: <span class="font-mono text-slate-100">{pct}%</span></span>
  <span class="block text-xs text-slate-500 mb-1">How much of the content score comes from resume similarity rather than your keyword list. 0% ignores the resume entirely.</span>
  <input type="range" min="0" max="100" step="5" name="resume_weight" value="{pct}"
    oninput="this.previousElementSibling.previousElementSibling.querySelector('span').textContent=this.value+'%'"
    class="w-full"/>
</label>"##,
        pct = (s.resume_weight * 100.0).round() as i64
    )
}

async fn settings_save(
    State(st): State<AppState>,
    Form(f): Form<SettingsForm>,
) -> impl IntoResponse {
    let mut s = (*st.settings().await).clone();

    if let Some(v) = &f.titles {
        s.titles = parse_list(v);
    }
    if let Some(v) = &f.keywords {
        s.keywords = parse_list(v);
    }
    if let Some(v) = &f.locations {
        s.locations = parse_list(v);
    }
    if let Some(v) = &f.seniority {
        s.seniority = parse_list(v);
    }
    if let Some(v) = &f.dealbreakers {
        s.dealbreakers = parse_list(v);
    }
    s.remote_ok = checked(&f.remote_ok);
    if let Some(v) = &f.location_policy {
        s.location_policy = v.trim().to_string();
    }
    s.allow_unknown_location = checked(&f.allow_unknown_location);
    s.min_salary = f
        .min_salary
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .and_then(|v| v.parse::<u64>().ok());

    s.per_hour_cap = num(&f.per_hour_cap, s.per_hour_cap);
    s.per_poster_cap = num(&f.per_poster_cap, s.per_poster_cap);
    s.score_floor = num(&f.score_floor, s.score_floor);
    s.strong_min = num(&f.strong_min, s.strong_min);
    s.exceptional_min = num(&f.exceptional_min, s.exceptional_min);
    s.settle_strong_secs = num(&f.settle_strong_mins, s.settle_strong_secs / 60) * 60;
    s.candidate_ttl_secs = num(&f.candidate_ttl_hours, s.candidate_ttl_secs / 3600) * 3600;
    s.adaptive_threshold = checked(&f.adaptive_threshold);
    s.adaptive_start = num(&f.adaptive_start, s.adaptive_start);
    s.adaptive_end = num(&f.adaptive_end, s.adaptive_end);

    s.greenhouse_enabled = checked(&f.greenhouse_enabled);
    if let Some(v) = &f.greenhouse_boards {
        s.greenhouse_boards = parse_list(v);
    }
    s.linkedin_guest_enabled = checked(&f.linkedin_guest_enabled);
    if let Some(v) = &f.linkedin_queries {
        // One query per line, "keywords | location".
        s.linkedin_queries = v
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(|l| {
                let (kw, loc) = l.split_once('|').unwrap_or((l, ""));
                LiQuery {
                    keywords: kw.trim().to_string(),
                    location: loc.trim().to_string(),
                }
            })
            .collect();
    }
    s.voyager_enabled = checked(&f.voyager_enabled);
    if let Some(v) = &f.voyager_queries {
        s.voyager_queries = parse_list(v);
    }
    if let Some(v) = &f.voyager_query_id {
        s.voyager_query_id = v.trim().to_string();
    }
    s.lookback_hours = num(&f.lookback_hours, s.lookback_hours);
    s.voyager_max_pages = num(&f.voyager_max_pages, s.voyager_max_pages);
    s.voyager_poll_pages = num(&f.voyager_poll_pages, s.voyager_poll_pages);
    // Set the mode LAST: sanitize() lets it overrule the individual toggles, so
    // reading it after them means the radio wins over stale checkbox state.
    if let Some(v) = &f.mode {
        s.mode = v.trim().to_string();
    }
    s.radar_hours = num(&f.radar_hours, s.radar_hours);
    // The slider posts 0..100 for usability; stored as a 0..1 fraction.
    if f.resume_weight.is_some() {
        s.resume_weight = num(&f.resume_weight, s.resume_weight * 100.0) / 100.0;
    }

    // Clamp before anything else sees it — the engine must never run on a
    // hand-posted form that inverts the tier ladder or zeroes the cap.
    s.sanitize();

    let note = match db::save_settings(&st.pool, &s).await {
        Ok(_) => {
            let resume = s.resume.clone();
            st.set_settings(s.clone()).await;
            st.rebuild_resume(&resume).await;
            st.notify_ui();
            tracing::info!("settings updated from dashboard");
            Some(Ok("Saved. Changes apply on the next crawl and the next release tick."))
        }
        Err(e) => {
            tracing::error!(%e, "settings save failed");
            Some(Err("Could not write settings to the database — nothing was changed."))
        }
    };

    // Re-render from what was actually stored, so clamped values are visible
    // rather than the raw numbers the user typed.
    Html(render_settings(&st, note).await)
}

fn ta(name: &str, label: &str, hint: &str, val: &[String], rows: usize) -> String {
    format!(
        r##"<label class="block">
  <span class="block text-sm text-slate-300">{label}</span>
  <span class="block text-xs text-slate-500 mb-1">{hint}</span>
  <textarea name="{name}" rows="{rows}"
    class="w-full rounded-md bg-slate-900/60 border border-slate-700 px-3 py-2 text-sm font-mono">{val}</textarea>
</label>"##,
        name = name,
        label = esc(label),
        hint = esc(hint),
        rows = rows,
        val = esc(&val.join("\n"))
    )
}

fn numf(name: &str, label: &str, val: String, step: &str) -> String {
    format!(
        r##"<label class="block">
  <span class="block text-sm text-slate-300">{label}</span>
  <input type="number" step="{step}" name="{name}" value="{val}"
    class="mt-1 w-full rounded-md bg-slate-900/60 border border-slate-700 px-3 py-1.5 text-sm font-mono"/>
</label>"##,
        name = name,
        label = esc(label),
        val = esc(&val),
        step = step
    )
}

/// Free-text input; numf() forces type=number, which the queryId is not.
fn textf(name: &str, label: &str, val: &str, hint: &str) -> String {
    format!(
        r##"<label class="block">
  <span class="block text-sm text-slate-300">{label}</span>
  <span class="block text-xs text-slate-500 mb-1">{hint}</span>
  <input type="text" name="{name}" value="{val}"
    class="w-full rounded-md bg-slate-900/60 border border-slate-700 px-3 py-1.5 text-sm font-mono"/>
</label>"##,
        name = name,
        label = esc(label),
        hint = esc(hint),
        val = esc(val)
    )
}

/// Presets over the three source toggles. "LinkedIn posts only" is the mode
/// this project was originally about; "job boards only" is the one that works
/// with no credentials and no ban risk.
fn mode_picker(s: &Settings) -> String {
    let opt = |val: &str, title: &str, desc: &str| {
        format!(
            r##"<label class="flex items-start gap-2 rounded-md ring-1 px-3 py-2 cursor-pointer {sel}">
  <input type="radio" name="mode" value="{val}" {on} class="mt-1"/>
  <span>
    <span class="block text-sm text-slate-200">{title}</span>
    <span class="block text-xs text-slate-500">{desc}</span>
  </span>
</label>"##,
            val = val,
            title = esc(title),
            desc = esc(desc),
            on = if s.mode == val { "checked" } else { "" },
            sel = if s.mode == val {
                "bg-slate-800/60 ring-slate-600"
            } else {
                "ring-slate-800 hover:ring-slate-700"
            }
        )
    };
    format!(
        r##"<div class="space-y-2">
  {all}
  {boards}
  {posts}
  {custom}
</div>"##,
        all = opt("all", "Everything", "Job boards and both LinkedIn sources."),
        boards = opt(
            "boards",
            "Job boards only",
            "Greenhouse. No login, no ban risk — the safe default."
        ),
        posts = opt(
            "posts",
            "LinkedIn posts only",
            "Hashtag searches of the feed for hiring posts. Needs your li_at cookie and a live queryId."
        ),
        custom = opt("custom", "Custom", "Use the individual toggles below."),
    )
}

/// How hard the location list bites.
///
/// This exists because "prefer" — the original and only behaviour — made
/// location worth ten points out of a hundred, so a strong match somewhere else
/// still cleared the floor and got alerted. Anyone who lists their cities means
/// it as a filter at least some of the time.
fn location_policy(s: &Settings) -> String {
    let opt = |val: &str, title: &str, desc: &str| {
        format!(
            r##"<label class="flex items-start gap-2 rounded-md ring-1 px-3 py-2 cursor-pointer {sel}">
  <input type="radio" name="location_policy" value="{val}" {on} class="mt-1"/>
  <span>
    <span class="block text-sm text-slate-200">{title}</span>
    <span class="block text-xs text-slate-500">{desc}</span>
  </span>
</label>"##,
            val = val,
            title = esc(title),
            desc = esc(desc),
            on = if s.location_policy == val { "checked" } else { "" },
            sel = if s.location_policy == val {
                "bg-slate-800/60 ring-slate-600"
            } else {
                "ring-slate-800 hover:ring-slate-700"
            }
        )
    };

    // Only meaningful under "require", so don't show it otherwise.
    let unknown = if s.location_policy == "require" {
        format!(
            r##"<div class="ml-3 mt-1">{}</div>"##,
            check(
                "allow_unknown_location",
                "Keep posts whose location I can't determine",
                s.allow_unknown_location
            )
        )
    } else {
        String::new()
    };

    format!(
        r##"<div class="space-y-2">
  <span class="block text-sm text-slate-300">Location matching</span>
  {require}
  {prefer}
  {off}
  {unknown}
</div>"##,
        require = opt(
            "require",
            "Only these locations",
            "Anything elsewhere is discarded outright, like a dealbreaker — this is what only-India actually means."
        ),
        prefer = opt(
            "prefer",
            "Prefer these locations",
            "A location hit adds points but never decides. A strong match elsewhere can still alert you."
        ),
        off = opt("off", "Ignore location", "Score on the work alone."),
        unknown = unknown,
    )
}

fn check(name: &str, label: &str, on: bool) -> String {
    format!(
        r##"<label class="flex items-center gap-2 py-1">
  <input type="checkbox" name="{name}" {on} class="rounded bg-slate-900 border-slate-600"/>
  <span class="text-sm text-slate-300">{label}</span>
</label>"##,
        name = name,
        label = esc(label),
        on = if on { "checked" } else { "" }
    )
}

fn settings_shell(s: &Settings, note: Option<Result<&str, &str>>, derived: &[String]) -> String {
    let banner = match note {
        Some(Ok(m)) => format!(
            r##"<div class="mb-4 rounded-md bg-emerald-500/10 ring-1 ring-emerald-500/30 text-emerald-300 px-3 py-2 text-sm">{}</div>"##,
            esc(m)
        ),
        Some(Err(m)) => format!(
            r##"<div class="mb-4 rounded-md bg-rose-500/10 ring-1 ring-rose-500/30 text-rose-300 px-3 py-2 text-sm">{}</div>"##,
            esc(m)
        ),
        None => String::new(),
    };

    let queries = s
        .linkedin_queries
        .iter()
        .map(|q| format!("{} | {}", q.keywords, q.location))
        .collect::<Vec<_>>();

    format!(
        r##"<!doctype html>
<html lang="en" class="dark">
<head>
  <meta charset="utf-8"/>
  <meta name="viewport" content="width=device-width, initial-scale=1"/>
  <title>Hiring Radar &middot; Settings</title>
  <script src="https://cdn.tailwindcss.com"></script>
</head>
<body class="bg-slate-950 text-slate-200 min-h-screen">
<div class="max-w-3xl mx-auto px-4 py-8">
  <header class="flex items-center justify-between mb-6">
    <div class="flex items-center gap-2">
      <span class="text-xl">&#128225;</span>
      <h1 class="text-lg font-semibold">Settings</h1>
    </div>
    <a href="/" class="text-xs text-slate-400 hover:text-slate-200">&larr; back to radar</a>
  </header>
  {banner}
  {resume}

  <form method="post" action="/settings" class="space-y-8">

    <section class="space-y-3">
      <h2 class="text-sm uppercase tracking-wide text-slate-400">What you're looking for</h2>
      {titles}
      {keywords}
      {locations}
      {seniority}
      {dealbreakers}
      {locpolicy}
      <div class="grid grid-cols-2 gap-3 items-end">
        {remote}
        {minsal}
      </div>
      {weight}
    </section>

    <section class="space-y-3">
      <h2 class="text-sm uppercase tracking-wide text-slate-400">Sources</h2>
      {mode}
      {gh_on}
      {gh_boards}
      {li_on}
      {li_queries}
      {voy_on}
      {voy_queries}
      {voy_qid}
      <div class="grid grid-cols-3 gap-3">
        {voy_hours}
        {voy_pages}
        {voy_poll}
      </div>
      <p class="text-xs text-slate-500">The first crawl walks back far enough to fill the window; every crawl after that only reads the newest page or two, because new posts are on page one and repeating the deep walk every 90 seconds is how accounts get banned.</p>
      <p class="text-xs text-slate-500">Your <code>li_at</code> cookie stays in <code>.env</code> &mdash; secrets don't belong in a web form. The queryId is not a secret, just a constant that breaks whenever LinkedIn ships, so it lives here.</p>
    </section>

    <section class="space-y-3">
      <h2 class="text-sm uppercase tracking-wide text-slate-400">Alert budget &amp; tiers</h2>
      <div class="grid grid-cols-2 sm:grid-cols-3 gap-3">
        {cap}
        {poster}
        {radar}
        {floor}
        {strong}
        {excep}
        {settle}
        {ttl}
      </div>
      <div class="pt-1">{adaptive}</div>
      <div class="grid grid-cols-2 gap-3">
        {astart}
        {aend}
      </div>
      <p class="text-xs text-slate-500">Values are clamped on save so the ladder can't invert: floor &lt; strong &lt; exceptional, and the per-poster cap can't exceed the hourly cap.</p>
    </section>

    <div class="flex items-center gap-3 pt-2">
      <button type="submit" class="rounded-md bg-emerald-500/90 hover:bg-emerald-400 px-4 py-2 text-sm font-medium text-slate-900">Save settings</button>
      <a href="/" class="text-sm text-slate-400 hover:text-slate-200">Cancel</a>
    </div>
  </form>
</div>
</body>
</html>"##,
        banner = banner,
        // resume_section renders its own form elements (upload, and remove when
        // a resume is loaded), so it is interpolated ABOVE the settings form and
        // must never move inside it. HTML forms cannot nest: a browser discards
        // the inner start tag but still acts on the inner closing tag, which
        // silently closes the outer form and orphans every field and button
        // after it. That is precisely how the Save button stopped working, and
        // it is invisible to any test that posts the form directly rather than
        // parsing the page. Guarded by save_button_is_inside_the_form_*.
        resume = resume_section(s, derived),
        weight = weight_slider(s),
        titles = ta("titles", "Target titles", "One per line. Matched against the job title; a full match is the strongest single signal.", &s.titles, 5),
        keywords = ta("keywords", "Skills / keywords", "One per line. Coverage across the post body.", &s.keywords, 5),
        locations = ta("locations", "Locations", "One per line. Matched against the listing's location field and its text \u{2014} and, for feed posts that state no location, against a place inferred from the post.", &s.locations, 4),
        seniority = ta("seniority", "Seniority", "One per line, e.g. senior, sde 2. Junior/intern titles are penalised when you want senior.", &s.seniority, 3),
        dealbreakers = ta("dealbreakers", "Dealbreakers", "One per line. Any hit zeroes the post outright — keep these specific.", &s.dealbreakers, 3),
        locpolicy = location_policy(s),
        remote = check("remote_ok", "Remote roles count as a location match", s.remote_ok),
        minsal = numf("min_salary", "Min salary (optional, blank = ignore)", s.min_salary.map(|v| v.to_string()).unwrap_or_default(), "1"),
        mode = mode_picker(s),
        gh_on = check("greenhouse_enabled", "Greenhouse boards", s.greenhouse_enabled),
        gh_boards = ta("greenhouse_boards", "Board tokens", "One per line — the slug in boards.greenhouse.io/<token>. A wrong token is a silent 404.", &s.greenhouse_boards, 4),
        li_on = check("linkedin_guest_enabled", "LinkedIn guest jobs", s.linkedin_guest_enabled),
        li_queries = ta("linkedin_queries", "LinkedIn queries", "One per line: keywords | location", &queries, 3),
        voy_on = check("voyager_enabled", "LinkedIn feed posts — authenticated, fragile, ban risk", s.voyager_enabled),
        voy_queries = ta("voyager_queries", "Feed searches", "One per line. Hashtags work best: #hiring, #hiringnow. Each runs as its own search.", &s.voyager_queries, 4),
        voy_hours = numf("lookback_hours", "Collect posts from the last (h)", s.lookback_hours.to_string(), "1"),
        voy_pages = numf("voyager_max_pages", "Pages on first fill", s.voyager_max_pages.to_string(), "1"),
        voy_poll = numf("voyager_poll_pages", "Pages when polling", s.voyager_poll_pages.to_string(), "1"),
        voy_qid = textf("voyager_query_id", "Voyager queryId", &s.voyager_query_id, "voyagerSearchDashClusters.xxxxxxxx — copy from DevTools → Network on a content search"),
        cap = numf("per_hour_cap", "Alerts / hour", s.per_hour_cap.to_string(), "1"),
        poster = numf("per_poster_cap", "Per company / hour", s.per_poster_cap.to_string(), "1"),
        radar = numf("radar_hours", "Radar window (h)", s.radar_hours.to_string(), "1"),
        floor = numf("score_floor", "Floor (drop below)", format!("{:.0}", s.score_floor), "1"),
        strong = numf("strong_min", "Strong at", format!("{:.0}", s.strong_min), "1"),
        excep = numf("exceptional_min", "Exceptional at", format!("{:.0}", s.exceptional_min), "1"),
        settle = numf("settle_strong_mins", "Settle strong (min)", (s.settle_strong_secs / 60).to_string(), "1"),
        ttl = numf("candidate_ttl_hours", "Keep competing (h)", (s.candidate_ttl_secs / 3600).to_string(), "1"),
        adaptive = check("adaptive_threshold", "Relax the bar as the hour drains", s.adaptive_threshold),
        astart = numf("adaptive_start", "Bar at :00", format!("{:.0}", s.adaptive_start), "1"),
        aend = numf("adaptive_end", "Bar at :59", format!("{:.0}", s.adaptive_end), "1"),
    )
}


#[cfg(test)]
mod tests {
    use super::*;

    fn page_with(resume: &str) -> String {
        let mut s: Settings = serde_json::from_str("{}").expect("all fields default");
        s.resume = resume.to_string();
        if !resume.is_empty() {
            s.resume_filename = Some("cv.pdf".into());
        }
        settings_shell(&s, None, &["golang".to_string()])
    }

    /// The bug this guards against was invisible to every curl test: posting the
    /// form directly worked fine, but in a browser the Save button was orphaned
    /// outside the form and did nothing.
    ///
    /// Nested forms are unrepresentable in the DOM — the parser drops the inner
    /// <form> start tag and honours its </form>, closing the outer one early.
    /// So: between the settings <form> and its submit button there must be no
    /// </form> at all.
    fn assert_save_button_is_inside_the_form(html: &str) {
        let open = html
            .find(r##"action="/settings""##)
            .expect("settings form is present");
        let save = html.find("Save settings").expect("save button is present");
        assert!(open < save, "form must open before the save button");
        let between = &html[open..save];
        assert!(
            !between.contains("</form>"),
            "a </form> between the settings form and its Save button orphans \
             every field after it:\n{}",
            between
                .match_indices("</form>")
                .map(|(i, _)| format!("at +{i}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        assert!(
            !between.contains("<form"),
            "a nested <form> inside the settings form breaks it"
        );
    }

    #[test]
    fn save_button_is_inside_the_form_without_a_resume() {
        assert_save_button_is_inside_the_form(&page_with(""));
    }

    /// With a resume loaded the block renders a second form (Remove resume), so
    /// this is the case that had two nested forms rather than one.
    #[test]
    fn save_button_is_inside_the_form_with_a_resume() {
        let long = "backend engineer golang kafka postgres ".repeat(20);
        assert_save_button_is_inside_the_form(&page_with(&long));
    }

    #[test]
    fn resume_upload_form_targets_its_own_endpoint() {
        let html = page_with("");
        assert!(
            html.contains(r##"action="/settings/resume""##),
            "upload form must post to its own endpoint"
        );
    }
}

// ===================== resume intake =====================

/// Accepts either an uploaded file or pasted text, whichever the multipart body
/// carries. PDF goes through a text-layer extractor: a scanned resume yields
/// nothing, and that is reported rather than silently storing an empty string
/// and leaving similarity scoring mysteriously inert.
async fn resume_upload(State(st): State<AppState>, mut mp: Multipart) -> impl IntoResponse {
    let mut text = String::new();
    let mut filename: Option<String> = None;
    let mut err: Option<String> = None;

    while let Ok(Some(field)) = mp.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        let fname = field.file_name().map(|s| s.to_string());

        match name.as_str() {
            "text" => {
                if let Ok(v) = field.text().await {
                    if !v.trim().is_empty() {
                        text = v;
                        filename = Some("pasted".into());
                    }
                }
            }
            "file" => {
                let Ok(bytes) = field.bytes().await else { continue };
                if bytes.is_empty() {
                    continue;
                }
                let looks_pdf = bytes.starts_with(b"%PDF");
                if looks_pdf {
                    // extract_text_from_mem panics on some malformed files
                    // rather than returning Err, so it runs inside catch_unwind.
                    let parsed = std::panic::catch_unwind(|| {
                        pdf_extract::extract_text_from_mem(&bytes)
                    });
                    match parsed {
                        Ok(Ok(t)) if t.trim().len() > 100 => {
                            text = t;
                            filename = fname;
                        }
                        Ok(Ok(_)) => {
                            err = Some(
                                "That PDF has no extractable text — it's probably a scan. \
                                 Paste the text instead."
                                    .into(),
                            );
                        }
                        _ => {
                            err = Some("Could not read that PDF. Paste the text instead.".into());
                        }
                    }
                } else {
                    match String::from_utf8(bytes.to_vec()) {
                        Ok(t) if t.trim().len() > 100 => {
                            text = t;
                            filename = fname;
                        }
                        Ok(_) => err = Some("That file looks empty.".into()),
                        Err(_) => {
                            err = Some(
                                "Only PDF and plain text are supported. For .docx, paste the text."
                                    .into(),
                            )
                        }
                    }
                }
            }
            _ => {}
        }
    }

    if text.trim().is_empty() {
        let msg: String =
            err.unwrap_or_else(|| "Nothing to read — choose a file or paste text.".into());
        return Html(render_settings(&st, Some(Err(&msg))).await);
    }

    let mut s = (*st.settings().await).clone();
    s.resume = text;
    s.resume_filename = filename;
    s.resume_updated_at = Some(crate::model::now());
    s.sanitize();

    let note = match db::save_settings(&st.pool, &s).await {
        Ok(_) => {
            let resume = s.resume.clone();
            st.set_settings(s.clone()).await;
            st.rebuild_resume(&resume).await;
            let m = st.matcher().await;
            match m.resume.as_ref() {
                Some(p) => Ok::<String, String>(format!(
                    "Resume loaded ({} characters). Top signals: {}.",
                    resume.len(),
                    p.top_terms(8).join(", ")
                )),
                None => Err("Read the file, but it was too short to build a profile from.".into()),
            }
        }
        Err(e) => {
            tracing::error!(%e, "resume save failed");
            Err("Could not write the resume to the database.".into())
        }
    };

    match note {
        Ok(m) => Html(render_settings(&st, Some(Ok(&m))).await),
        Err(m) => Html(render_settings(&st, Some(Err(&m))).await),
    }
}

async fn resume_clear(State(st): State<AppState>) -> impl IntoResponse {
    let mut s = (*st.settings().await).clone();
    s.resume.clear();
    s.resume_filename = None;
    s.resume_updated_at = None;
    let _ = db::save_settings(&st.pool, &s).await;
    st.set_settings(s).await;
    st.rebuild_resume("").await;
    Html(render_settings(&st, Some(Ok("Resume cleared — scoring is back to your keyword list."))).await)
}

fn resume_section(s: &Settings, derived: &[String]) -> String {
    let status = if s.resume.trim().is_empty() {
        r##"<p class="text-sm text-slate-400">No resume loaded. Scoring is using your keyword list.</p>"##.to_string()
    } else {
        format!(
            r##"<div class="rounded-md bg-slate-900/60 ring-1 ring-slate-700/60 p-3 space-y-2">
  <p class="text-sm text-slate-300">Loaded: <span class="font-mono text-slate-100">{name}</span>
     <span class="text-slate-500">&middot; {chars} characters</span></p>
  <p class="text-xs text-slate-500">Signals derived from it: <span class="text-slate-400">{terms}</span></p>
  <form method="post" action="/settings/resume/clear">
    <button type="submit" class="text-xs text-rose-400 hover:text-rose-300">Remove resume</button>
  </form>
</div>"##,
            name = esc(s.resume_filename.as_deref().unwrap_or("resume")),
            chars = s.resume.len(),
            terms = if derived.is_empty() {
                "(not enough text to build a profile)".to_string()
            } else {
                esc(&derived.join(", "))
            }
        )
    };

    format!(
        r##"<section class="space-y-3">
  <h2 class="text-sm uppercase tracking-wide text-slate-400">Your resume</h2>
  <p class="text-xs text-slate-500">With a resume loaded, posts are scored by how much they look like
     <em>your</em> work &mdash; IDF-weighted similarity over the posts this instance has seen &mdash;
     instead of by a hand-maintained keyword list. Nothing leaves the container.</p>
  {status}
  <form method="post" action="/settings/resume" enctype="multipart/form-data" class="space-y-2">
    <label class="block">
      <span class="block text-sm text-slate-300">Upload a PDF or .txt</span>
      <input type="file" name="file" accept=".pdf,.txt,.md,text/plain,application/pdf"
        class="mt-1 block w-full text-sm text-slate-400 file:mr-3 file:rounded-md file:border-0 file:bg-slate-700 file:px-3 file:py-1.5 file:text-sm file:text-slate-200"/>
      <span class="block text-xs text-slate-500 mt-1">Text-layer PDFs only &mdash; a scan has nothing to read. For .docx, paste below.</span>
    </label>
    <label class="block">
      <span class="block text-sm text-slate-300">&hellip; or paste the text</span>
      <textarea name="text" rows="6" placeholder="Paste your resume here"
        class="w-full rounded-md bg-slate-900/60 border border-slate-700 px-3 py-2 text-sm"></textarea>
    </label>
    <button type="submit" class="rounded-md bg-sky-500/90 hover:bg-sky-400 px-3 py-1.5 text-sm font-medium text-slate-900">Load resume</button>
  </form>
</section>"##,
        status = status
    )
}


// ===================== status =====================

/// The panel that answers "why is this empty".
///
/// An empty radar has several causes that look identical from the outside —
/// first crawl still running, source switched off, board tokens 404ing, or
/// everything scoring below the floor and being discarded. Reading container
/// logs to tell them apart defeats the point of a dashboard, so the state is
/// on the page.
async fn render_status(st: &AppState, hours: i64) -> String {
    let settings = st.settings().await;
    let snap = st.status_snapshot().await;
    let rows_in_window = db::recent_count(&st.pool, hours * 3600).await.unwrap_or(0);
    let (level, message) = diagnosis(&snap, settings.score_floor, rows_in_window);

    let (banner_class, icon) = match level {
        Level::Ok => ("bg-emerald-500/10 ring-emerald-500/30 text-emerald-300", "&check;"),
        Level::Info => ("bg-sky-500/10 ring-sky-500/30 text-sky-300", "&hellip;"),
        Level::Warn => ("bg-amber-500/10 ring-amber-500/30 text-amber-300", "!"),
        Level::Error => ("bg-rose-500/10 ring-rose-500/30 text-rose-300", "&times;"),
    };

    let sources = if snap.sources.is_empty() {
        r##"<p class="text-xs text-slate-500">Starting up&hellip;</p>"##.to_string()
    } else {
        snap.sources
            .iter()
            .map(|(name, s)| {
                let dot = if !s.enabled {
                    "bg-slate-700"
                } else if s.last_error.is_some() {
                    "bg-rose-400"
                } else if s.running {
                    "bg-sky-400 animate-pulse"
                } else {
                    "bg-emerald-400"
                };

                let when = if !s.enabled {
                    "off".to_string()
                } else if s.running {
                    "crawling now".to_string()
                } else {
                    match (s.last_run, s.next_run) {
                        (Some(l), Some(n)) => format!("{} &middot; next {}", since(l), until(n)),
                        (Some(l), None) => since(l),
                        _ => "waiting for first crawl".to_string(),
                    }
                };

                // Last cycle, in the terms that explain an empty board.
                let counts = if s.last_run.is_none() {
                    String::new()
                } else {
                    format!(
                        r##"<span class="text-slate-500">{fetched} fetched &middot; {new} new &middot; {stored} kept{dropped}{wrongloc}{notmatch}</span>"##,
                        fetched = s.fetched,
                        new = s.new_posts,
                        stored = s.stored,
                        dropped = if s.below_floor > 0 {
                            format!(" &middot; {} below floor", s.below_floor)
                        } else {
                            String::new()
                        },
                        notmatch = if s.not_hiring > 0 {
                            format!(" &middot; {} not hiring posts", s.not_hiring)
                        } else {
                            String::new()
                        },
                        wrongloc = if s.wrong_location > 0 {
                            format!(" &middot; {} wrong location", s.wrong_location)
                        } else {
                            String::new()
                        },
                    )
                };

                let notes = if s.notes.is_empty() {
                    String::new()
                } else {
                    format!(
                        r##"<ul class="mt-1 ml-4 space-y-0.5">{}</ul>"##,
                        s.notes
                            .iter()
                            .map(|n| {
                                format!(
                                    r##"<li class="text-xs {}">{}</li>"##,
                                    if n.ok { "text-slate-600" } else { "text-amber-400" },
                                    esc(&n.text)
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("")
                    )
                };

                let err = match &s.last_error {
                    Some(e) => format!(
                        r##"<p class="mt-1 ml-4 text-xs text-rose-400">{}</p>"##,
                        esc(e)
                    ),
                    None => String::new(),
                };

                format!(
                    r##"<li class="py-1.5 border-b border-slate-800/60 last:border-0">
  <div class="flex items-center gap-2 text-sm">
    <span class="w-1.5 h-1.5 rounded-full shrink-0 {dot}"></span>
    <span class="text-slate-300 font-mono text-xs">{name}</span>
    <span class="text-xs text-slate-500">{when}</span>
  </div>
  <div class="ml-4 text-xs">{counts}</div>
  {notes}
  {err}
</li>"##,
                    dot = dot,
                    name = esc(name),
                    when = when,
                    counts = counts,
                    notes = notes,
                    err = err
                )
            })
            .collect::<Vec<_>>()
            .join("")
    };

    // Set expectations explicitly. The first crawl deliberately does not alert,
    // and someone watching a quiet board should know that is by design.
    let expectation = if snap.sources.values().any(|s| s.bootstrapped) {
        format!(
            "Alerts fire only for postings that appear <em>after</em> the first crawl — \
             history is on the radar but was never alerted, by design. \
             Budget: {} alerts/hour, tiers at {:.0}/{:.0}/{:.0}.",
            settings.per_hour_cap,
            settings.score_floor,
            settings.strong_min,
            settings.exceptional_min
        )
    } else {
        "The first crawl of each source records what is already posted without \
         alerting, so you are not blasted with history. Anything posted after \
         that can fire."
            .to_string()
    };

    format!(
        r##"<section id="status" class="mb-6">
  <div class="flex items-center justify-between mb-2">
    <h2 class="text-sm uppercase tracking-wide text-slate-400">Status</h2>
    <a href="/settings" class="text-xs text-slate-500 hover:text-slate-300">tune &rarr;</a>
  </div>
  <div class="rounded-md ring-1 px-3 py-2 text-sm {banner_class}">
    <span class="font-mono mr-1">{icon}</span>{message}
  </div>
  <ul class="mt-2">{sources}</ul>
  <p class="mt-2 text-xs text-slate-600">{expectation}</p>
</section>"##,
        banner_class = banner_class,
        icon = icon,
        message = esc(&message),
        sources = sources,
        expectation = expectation
    )
}
