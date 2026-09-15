//! HTTP: the React bundle, the event stream, and the resume upload's shared
//! plumbing.
//!
//! The dashboard used to be rendered here as htmx fragments. It is React now
//! (see `frontend/`), talking to the JSON surface in `api`, so this file is
//! down to the three things that are not the API:
//!
//! - serving the built single-page app, including the fallback that tells you
//!   how to build it when it isn't there
//! - the server-sent event stream that tells an open tab something changed
//! - resume intake, which is multipart rather than JSON because it carries a
//!   file, and is shared with the API endpoint

use crate::db;
use crate::state::AppState;
use axum::extract::{DefaultBodyLimit, Multipart, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::Router;
use futures::stream::StreamExt;
use std::convert::Infallible;
use std::path::PathBuf;
use tokio_stream::wrappers::BroadcastStream;
use tower_http::services::{ServeDir, ServeFile};

/// Where the built frontend lives. Overridable so the container can serve it
/// from wherever the image puts it, and so `npm run dev` isn't the only way to
/// work on it.
fn ui_dir() -> PathBuf {
    PathBuf::from(std::env::var("RADAR_UI_DIR").unwrap_or_else(|_| "frontend/dist".into()))
}

pub fn router(state: AppState) -> Router {
    let dir = ui_dir();
    let index = dir.join("index.html");

    let api = Router::new()
        .route("/events", get(events))
        .route(
            "/settings/resume",
            // A PDF resume comfortably exceeds axum's 2MB default.
            post(resume_upload).layer(DefaultBodyLimit::max(20 * 1024 * 1024)),
        )
        .merge(crate::api::routes())
        .with_state(state);

    if index.is_file() {
        // The SPA owns its own routing, so any path that isn't a real file
        // falls back to index.html — otherwise a reload on a deep link 404s.
        api.fallback_service(ServeDir::new(&dir).fallback(ServeFile::new(&index)))
    } else {
        // A 404 at the root would read as "the server is broken". It isn't —
        // the bundle just hasn't been built, which is a one-line fix worth
        // stating instead of leaving someone to guess.
        tracing::warn!(dir = %dir.display(), "no built frontend; serving the build instructions");
        api.fallback(missing_ui)
    }
}

async fn missing_ui() -> impl IntoResponse {
    (StatusCode::SERVICE_UNAVAILABLE, Html(build_instructions()))
}

fn build_instructions() -> String {
    format!(
        r##"<!doctype html>
<html lang="en"><head><meta charset="utf-8"/>
<title>Hiring Radar &middot; no UI built</title>
<style>
 body{{background:#020617;color:#e2e8f0;font:14px ui-sans-serif,system-ui,sans-serif;margin:0;padding:3rem 1.5rem}}
 main{{max-width:34rem;margin:0 auto}} code{{background:#0f172a;padding:.15rem .4rem;border-radius:.25rem}}
 pre{{background:#0f172a;padding:.75rem 1rem;border-radius:.5rem;overflow-x:auto}}
 a{{color:#7dd3fc}} h1{{font-size:1.1rem}} p{{line-height:1.6;color:#94a3b8}}
</style></head><body><main>
<h1>&#128225; The dashboard hasn't been built yet</h1>
<p>The API is up &mdash; <a href="/api/status">/api/status</a> answers &mdash; but there is no
front-end bundle at <code>{dir}</code>.</p>
<pre>cd frontend &amp;&amp; npm install &amp;&amp; npm run build</pre>
<p>Then reload. The Docker image builds this for you, so this message means you're running the
binary directly. Set <code>RADAR_UI_DIR</code> if the bundle lives somewhere else.</p>
</main></body></html>"##,
        dir = ui_dir().display()
    )
}

/// SSE stream: every queue change pushes a tick; the open tab refetches.
///
/// A tick rather than the changed data, on purpose. The client knows which
/// filters it is showing and the server does not, so "something changed, ask
/// me" is the only message that cannot be wrong.
async fn events(
    State(st): State<AppState>,
) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    let rx = st.events.subscribe();
    let stream = BroadcastStream::new(rx).map(|_| Ok(Event::default().data("tick")));
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// A short label for a source. Lives here rather than in the client so two
/// surfaces cannot disagree about what a source is called.
pub fn source_label(source: &str) -> &str {
    match source {
        "greenhouse" => "greenhouse",
        "linkedin_guest" => "li\u{b7}jobs",
        "linkedin_voyager" => "li\u{b7}posts",
        "lever" => "lever",
        "workday" => "workday",
        other => other,
    }
}

// ===================== resume intake =====================

/// Pull the resume text out of an upload, whether it arrived as a PDF, a text
/// file, or pasted into the textarea.
///
/// Shared by the HTML and JSON upload endpoints: PDF extraction is the fiddly
/// part (scans have no text layer, and the extractor panics on some malformed
/// files), and having two copies of it means one of them keeps a bug the other
/// one fixed.
pub(crate) async fn read_resume_upload(
    mut mp: Multipart,
) -> Result<(String, Option<String>), String> {
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
                        Ok(Err(e)) => err = Some(format!("Couldn't read that PDF — {e}")),
                        Err(_) => {
                            err = Some("Couldn't read that PDF — it may be corrupt.".into())
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
        return Err(err.unwrap_or_else(|| "Nothing to read — choose a file or paste text.".into()));
    }
    Ok((text, filename))
}

/// Store an extracted resume and re-vectorise it. Returns the line to show.
pub(crate) async fn store_resume(
    st: &AppState,
    text: String,
    filename: Option<String>,
) -> Result<String, String> {
    let mut s = (*st.settings().await).clone();
    s.resume = text;
    s.resume_filename = filename;
    s.resume_updated_at = Some(crate::model::now());
    s.sanitize();

    db::save_settings(&st.pool, &s)
        .await
        .map_err(|e| format!("Couldn't save: {e}"))?;

    let resume = s.resume.clone();
    st.set_settings(s.clone()).await;
    st.rebuild_resume(&resume).await;
    // The ATS scores against the profile, so it has to move with the resume —
    // otherwise a fresh upload changes the similarity score and leaves every
    // structured comparison reading the old CV.
    st.rebuild_profile(&s).await;
    // A new resume changes what every score on the board means.
    if let Err(e) = crate::pipeline::rescore_all(st).await {
        tracing::warn!(%e, "re-score after resume upload failed");
    }
    let m = st.matcher().await;
    match m.resume.as_ref() {
        Some(p) => Ok(format!(
            "Resume loaded ({} characters). Top signals: {}.",
            resume.len(),
            p.top_terms(8).join(", ")
        )),
        None => Err("Read the file, but it was too short to build a profile from.".into()),
    }
}

/// The plain-form resume endpoint.
///
/// Kept alongside the JSON one because a file input posting multipart is the
/// one thing a form still does better than fetch, and because it works with
/// JavaScript off — a reasonable thing for a resume upload to do.
async fn resume_upload(
    State(st): State<AppState>,
    mp: Multipart,
) -> impl IntoResponse {
    match read_resume_upload(mp).await {
        Err(msg) => (StatusCode::UNPROCESSABLE_ENTITY, Html(note_page(&msg, false))).into_response(),
        Ok((text, filename)) => match store_resume(&st, text, filename).await {
            Ok(msg) => {
                st.notify_ui();
                Html(note_page(&msg, true)).into_response()
            }
            Err(msg) => {
                (StatusCode::UNPROCESSABLE_ENTITY, Html(note_page(&msg, false))).into_response()
            }
        },
    }
}

/// The message is server-generated, but it can quote a filename the user chose,
/// so it is escaped anyway. Escaping only what you think is untrusted is how
/// something untrusted eventually gets through.
fn note_page(msg: &str, ok: bool) -> String {
    format!(
        r##"<!doctype html><html lang="en"><head><meta charset="utf-8"/>
<title>Hiring Radar</title><style>
 body{{background:#020617;color:{colour};font:14px ui-sans-serif,system-ui,sans-serif;margin:0;padding:3rem 1.5rem}}
 main{{max-width:34rem;margin:0 auto}} a{{color:#7dd3fc}}
</style></head><body><main><p>{msg}</p><p><a href="/#/settings">&larr; back to settings</a></p></main></body></html>"##,
        colour = if ok { "#6ee7b7" } else { "#fda4af" },
        msg = msg
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_labels_are_short_and_stable() {
        // The client renders these verbatim, so a rename here is a rename in
        // both surfaces at once — which is why it lives server-side.
        assert_eq!(source_label("linkedin_voyager"), "li\u{b7}posts");
        assert_eq!(source_label("workday"), "workday");
        // An unknown source shows its own name rather than "unknown".
        assert_eq!(source_label("lever"), "lever");
    }

    #[test]
    fn the_no_ui_page_says_exactly_how_to_fix_it() {
        let html = build_instructions();
        assert!(html.contains("npm run build"), "{html}");
        assert!(html.contains("RADAR_UI_DIR"), "{html}");
    }

    #[test]
    fn a_message_cannot_smuggle_markup_into_the_note_page() {
        let html = note_page("<script>alert(1)</script>", false);
        assert!(!html.contains("<script>alert"), "{html}");
        assert!(html.contains("&lt;script&gt;"), "{html}");
    }
}
