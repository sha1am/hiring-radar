use crate::db;
use crate::model::Candidate;
use crate::notify;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Form, Router};
use futures::stream::StreamExt;
use serde::Deserialize;
use std::convert::Infallible;
use tokio_stream::wrappers::BroadcastStream;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(page))
        .route("/queue", get(queue))
        .route("/events", get(events))
        .route("/candidate/:id/send", post(send))
        .route("/candidate/:id/dismiss", post(dismiss))
        .route("/candidate/:id/applied", post(applied))
        .route("/candidate/:id/draft", post(save_draft))
        .with_state(state)
}

async fn page(State(st): State<AppState>) -> impl IntoResponse {
    let q = render_queue(&st).await;
    let h = render_history(&st).await;
    Html(shell(&q, &h))
}

async fn queue(State(st): State<AppState>) -> impl IntoResponse {
    Html(render_queue(&st).await)
}

/// SSE stream: every queue change pushes a tick; the browser reloads #queue.
async fn events(State(st): State<AppState>) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
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

async fn save_draft(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    Form(f): Form<DraftForm>,
) -> impl IntoResponse {
    let _ = db::set_draft(&st.pool, id, &f.subject, &f.body).await;
    Html("saved".to_string())
}

// ----- rendering -----

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn tier_badge(tier: &str) -> &'static str {
    match tier {
        "exceptional" => "bg-rose-500/15 text-rose-300 ring-rose-500/30",
        "strong" => "bg-amber-500/15 text-amber-300 ring-amber-500/30",
        _ => "bg-slate-500/15 text-slate-300 ring-slate-500/30",
    }
}

fn card(c: &Candidate) -> String {
    let draft_subject = esc(c.draft_subject.as_deref().unwrap_or(""));
    let draft_body = esc(c.draft_body.as_deref().unwrap_or(""));
    let loc = esc(&c.location.clone().unwrap_or_default());

    // Action row depends on how you actually reach this poster.
    let actions = match c.apply_kind.as_str() {
        "email" => format!(
            r#"<button type="submit"
                 class="rounded-md bg-emerald-500/90 hover:bg-emerald-400 px-3 py-1.5 text-sm font-medium text-slate-900">
                 Send email → {to}</button>"#,
            to = esc(&c.apply_target.clone().unwrap_or_default())
        ),
        "dm" => format!(
            r#"<button type="button" onclick="copyBody({id})"
                 class="rounded-md bg-sky-500/90 hover:bg-sky-400 px-3 py-1.5 text-sm font-medium text-slate-900">Copy draft</button>
               <a target="_blank" href="{url}"
                 class="rounded-md bg-slate-700 hover:bg-slate-600 px-3 py-1.5 text-sm">Open profile ↗</a>"#,
            id = c.id, url = esc(&c.url)
        ),
        _ => format!(
            r#"<a target="_blank" href="{url}"
                 class="rounded-md bg-sky-500/90 hover:bg-sky-400 px-3 py-1.5 text-sm font-medium text-slate-900">Open apply page ↗</a>"#,
            url = esc(&c.url)
        ),
    };

    let subject_field = if c.apply_kind == "email" {
        format!(
            r#"<input name="subject" value="{subj}"
                 class="w-full rounded-md bg-slate-900/60 border border-slate-700 px-3 py-1.5 text-sm mb-2"/>"#,
            subj = draft_subject
        )
    } else {
        String::new()
    };

    format!(
        r##"<article id="card-{id}" class="rounded-xl bg-slate-800/60 ring-1 ring-slate-700/60 p-4">
  <div class="flex items-start justify-between gap-3">
    <div>
      <h3 class="font-semibold text-slate-100">{title}</h3>
      <p class="text-sm text-slate-400">{company} · {loc} · {age}</p>
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
        r#"<p class="text-slate-500 text-sm py-8 text-center">No live matches. The radar is watching…</p>"#.to_string()
    } else {
        items.iter().map(card).collect::<Vec<_>>().join("\n")
    };

    format!(
        r#"<section id="queue" class="space-y-3">
  <div class="flex items-center justify-between">
    <h2 class="text-sm uppercase tracking-wide text-slate-400">Awaiting you</h2>
    <span class="text-xs text-slate-500">{used}/{cap} sent this hour</span>
  </div>
  {cards}
</section>"#,
        used = used,
        cap = cap,
        cards = cards
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
                r#"<li class="flex items-center justify-between py-1.5 border-b border-slate-800/60">
                    <span class="truncate text-slate-300">{title} · <span class="text-slate-500">{company}</span></span>
                    <span class="{color} text-xs uppercase ml-3 shrink-0">{status}</span>
                   </li>"#,
                title = esc(&c.title),
                company = esc(&c.company),
                status = c.status,
                color = color
            )
        })
        .collect::<Vec<_>>()
        .join("");
    format!(
        r#"<section class="mt-8">
  <h2 class="text-sm uppercase tracking-wide text-slate-400 mb-2">Recent</h2>
  <ul class="text-sm">{rows}</ul>
</section>"#,
        rows = rows
    )
}

fn shell(queue: &str, history: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="en" class="dark">
<head>
  <meta charset="utf-8"/>
  <meta name="viewport" content="width=device-width, initial-scale=1"/>
  <title>Hiring Radar</title>
  <script src="https://cdn.tailwindcss.com"></script>
  <script src="https://unpkg.com/htmx.org@1.9.12"></script>
</head>
<body class="bg-slate-950 text-slate-200 min-h-screen">
  <div class="max-w-2xl mx-auto px-4 py-8">
    <header class="flex items-center justify-between mb-6">
      <div class="flex items-center gap-2">
        <span class="text-xl">📡</span>
        <h1 class="text-lg font-semibold">Hiring Radar</h1>
        <span id="live" class="ml-2 inline-flex items-center gap-1 text-xs text-emerald-400">
          <span class="w-1.5 h-1.5 rounded-full bg-emerald-400 animate-pulse"></span>live</span>
      </div>
      <button onclick="reloadQueue()" class="text-xs text-slate-400 hover:text-slate-200">refresh</button>
    </header>
    {queue}
    {history}
  </div>

  <script>
    function reloadQueue() {{ htmx.ajax('GET', '/queue', {{target:'#queue', swap:'outerHTML'}}); }}
    function copyBody(id) {{
      const el = document.getElementById('body-' + id);
      navigator.clipboard.writeText(el.value);
    }}
    // Live updates: the server pings on every queue change; we refetch the list.
    const es = new EventSource('/events');
    es.onmessage = () => reloadQueue();
    es.onerror = () => {{ document.getElementById('live').style.opacity = 0.4; }};
  </script>
</body>
</html>"#,
        queue = queue,
        history = history
    )
}
