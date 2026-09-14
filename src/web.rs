use crate::db;
use crate::model::Candidate;
use crate::notify;
use crate::state::AppState;
use axum::extract::{Path, Query, State};
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
        .route("/events", get(events))
        .route("/candidate/:id/send", post(send))
        .route("/candidate/:id/dismiss", post(dismiss))
        .route("/candidate/:id/applied", post(applied))
        .route("/candidate/:id/draft", post(save_draft))
        .with_state(state)
}

async fn page(State(st): State<AppState>) -> impl IntoResponse {
    let hours = st.cfg.server.radar_hours;
    let q = render_queue(&st).await;
    let r = render_radar(&st, hours).await;
    let h = render_history(&st).await;
    Html(shell(&q, &r, &h))
}

async fn queue(State(st): State<AppState>) -> impl IntoResponse {
    Html(render_queue(&st).await)
}

#[derive(Deserialize)]
struct RadarQuery {
    hours: Option<i64>,
}

async fn radar(State(st): State<AppState>, Query(q): Query<RadarQuery>) -> impl IntoResponse {
    let hours = q.hours.unwrap_or(st.cfg.server.radar_hours).clamp(1, 24 * 30);
    Html(render_radar(&st, hours).await)
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
    let cap = st.cfg.release.per_hour_cap;
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
    <span class="block truncate text-xs text-slate-500">{company}{loc} &middot; {source}</span>
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
        age = esc(&c.age_str()),
        chip_class = chip_class,
        chip = chip,
    )
}

fn window_button(hours: i64, active: i64, label: &str) -> String {
    let cls = if hours == active {
        "bg-slate-700 text-slate-100"
    } else {
        "text-slate-500 hover:text-slate-300"
    };
    format!(
        r##"<button hx-get="/radar?hours={hours}" hx-target="#radar" hx-swap="outerHTML"
      class="px-2 py-0.5 rounded text-xs {cls}">{label}</button>"##,
        hours = hours,
        cls = cls,
        label = label
    )
}

/// Everything seen in the trailing window, not just what fired. This is the
/// "what's out there" half of the dashboard — history included, so the board is
/// useful on the very first run instead of empty until something new posts.
async fn render_radar(st: &AppState, hours: i64) -> String {
    let window = hours * 3600;
    let mut items = db::recent(&st.pool, window, 300).await.unwrap_or_default();
    let total = db::recent_count(&st.pool, window).await.unwrap_or(0);

    // Best match first — the list is for scanning, not for chronology.
    items.sort_by(|a, b| {
        b.live_priority()
            .partial_cmp(&a.live_priority())
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let rows = if items.is_empty() {
        r##"<p class="text-slate-500 text-sm py-6 text-center">Nothing in this window yet. Widen it, or give the first crawl a minute.</p>"##.to_string()
    } else {
        format!(
            r##"<ul class="mt-1">{}</ul>"##,
            items.iter().map(radar_row).collect::<Vec<_>>().join("")
        )
    };

    let shown = if total > items.len() as i64 {
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
  {rows}
</section>"##,
        total = total,
        hours = hours,
        shown = shown,
        b6 = window_button(6, hours, "6h"),
        b24 = window_button(24, hours, "24h"),
        b72 = window_button(72, hours, "3d"),
        b168 = window_button(168, hours, "7d"),
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

fn shell(queue: &str, radar: &str, history: &str) -> String {
    format!(
        r##"<!doctype html>
<html lang="en" class="dark">
<head>
  <meta charset="utf-8"/>
  <meta name="viewport" content="width=device-width, initial-scale=1"/>
  <title>Hiring Radar</title>
  <script src="https://cdn.tailwindcss.com"></script>
  <script src="https://unpkg.com/htmx.org@1.9.12"></script>
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
      <button onclick="reloadAll()" class="text-xs text-slate-400 hover:text-slate-200">refresh</button>
    </header>
    {queue}
    {radar}
    {history}
  </div>

  <script>
    function reloadAll() {{
      htmx.ajax('GET', '/queue', {{target:'#queue', swap:'outerHTML'}});
      const active = document.querySelector('#radar button.bg-slate-700');
      const hours = active ? new URL(active.getAttribute('hx-get'), location.origin).searchParams.get('hours') : '24';
      htmx.ajax('GET', '/radar?hours=' + hours, {{target:'#radar', swap:'outerHTML'}});
      htmx.ajax('GET', '/', {{target:'#history', swap:'none'}});
    }}
    function copyBody(id) {{
      const el = document.getElementById('body-' + id);
      navigator.clipboard.writeText(el.value);
    }}
    // Live updates: the server pings on every queue change; we refetch the lists.
    const es = new EventSource('/events');
    es.onmessage = () => reloadAll();
    es.onerror = () => {{ document.getElementById('live').style.opacity = 0.4; }};
  </script>
</body>
</html>"##,
        queue = queue,
        radar = radar,
        history = history
    )
}
