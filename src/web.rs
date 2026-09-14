use crate::db;
use crate::model::Candidate;
use crate::notify;
use crate::settings::{parse_list, Query as LiQuery, Settings};
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
        .route("/settings", get(settings_page).post(settings_save))
        .with_state(state)
}

async fn page(State(st): State<AppState>) -> impl IntoResponse {
    let hours = st.settings().await.radar_hours;
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
    let default_hours = st.settings().await.radar_hours;
    let hours = q.hours.unwrap_or(default_hours).clamp(1, 24 * 30);
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
      <div class="flex items-center gap-3">
        <button onclick="reloadAll()" class="text-xs text-slate-400 hover:text-slate-200">refresh</button>
        <a href="/settings" class="text-xs text-slate-400 hover:text-slate-200">settings</a>
      </div>
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

    greenhouse_enabled: Option<String>,
    greenhouse_boards: Option<String>,
    linkedin_guest_enabled: Option<String>,
    linkedin_queries: Option<String>,
    voyager_enabled: Option<String>,

    radar_hours: Option<String>,
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

async fn settings_page(State(st): State<AppState>) -> impl IntoResponse {
    let s = st.settings().await;
    Html(settings_shell(&s, None))
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
    s.radar_hours = num(&f.radar_hours, s.radar_hours);

    // Clamp before anything else sees it — the engine must never run on a
    // hand-posted form that inverts the tier ladder or zeroes the cap.
    s.sanitize();

    let note = match db::save_settings(&st.pool, &s).await {
        Ok(_) => {
            st.set_settings(s.clone()).await;
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
    let shown = st.settings().await;
    Html(settings_shell(&shown, note))
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

fn settings_shell(s: &Settings, note: Option<Result<&str, &str>>) -> String {
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
  <form method="post" action="/settings" class="space-y-8">

    <section class="space-y-3">
      <h2 class="text-sm uppercase tracking-wide text-slate-400">What you're looking for</h2>
      {titles}
      {keywords}
      {locations}
      {seniority}
      {dealbreakers}
      <div class="grid grid-cols-2 gap-3 items-end">
        {remote}
        {minsal}
      </div>
    </section>

    <section class="space-y-3">
      <h2 class="text-sm uppercase tracking-wide text-slate-400">Sources</h2>
      {gh_on}
      {gh_boards}
      {li_on}
      {li_queries}
      {voy_on}
      <p class="text-xs text-slate-500">Voyager also needs enabled credentials (LI_AT) and a live queryId in config.toml &mdash; secrets don't belong in a web form.</p>
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
        titles = ta("titles", "Target titles", "One per line. Matched against the job title; a full match is the strongest single signal.", &s.titles, 5),
        keywords = ta("keywords", "Skills / keywords", "One per line. Coverage across the post body.", &s.keywords, 5),
        locations = ta("locations", "Locations", "One per line. Matched against the listing location and body.", &s.locations, 4),
        seniority = ta("seniority", "Seniority", "One per line, e.g. senior, sde 2. Junior/intern titles are penalised when you want senior.", &s.seniority, 3),
        dealbreakers = ta("dealbreakers", "Dealbreakers", "One per line. Any hit zeroes the post outright — keep these specific.", &s.dealbreakers, 3),
        remote = check("remote_ok", "Remote roles count as a location match", s.remote_ok),
        minsal = numf("min_salary", "Min salary (optional, blank = ignore)", s.min_salary.map(|v| v.to_string()).unwrap_or_default(), "1"),
        gh_on = check("greenhouse_enabled", "Greenhouse boards", s.greenhouse_enabled),
        gh_boards = ta("greenhouse_boards", "Board tokens", "One per line — the slug in boards.greenhouse.io/<token>. A wrong token is a silent 404.", &s.greenhouse_boards, 4),
        li_on = check("linkedin_guest_enabled", "LinkedIn guest jobs", s.linkedin_guest_enabled),
        li_queries = ta("linkedin_queries", "LinkedIn queries", "One per line: keywords | location", &queries, 3),
        voy_on = check("voyager_enabled", "LinkedIn Voyager (authenticated, fragile, ban risk)", s.voyager_enabled),
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
