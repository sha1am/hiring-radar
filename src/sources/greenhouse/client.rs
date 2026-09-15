//! The Greenhouse boards API: one call, one wire format.

use crate::sources::common::{brief, status_hint};
use serde::Deserialize;

/// Where the board API lives. Overridable so the pipeline can be exercised
/// against a fixture server — there is no other way to test ingest end to end
/// without hitting Greenhouse for real.
pub const DEFAULT_BASE: &str = "https://boards-api.greenhouse.io/v1/boards";

pub struct Client {
    http: reqwest::Client,
    base: String,
}

#[derive(Deserialize)]
pub struct Resp {
    pub jobs: Vec<Job>,
}

#[derive(Deserialize)]
pub struct Job {
    pub id: i64,
    pub title: String,
    pub absolute_url: String,
    #[serde(default)]
    pub location: Option<Loc>,
    #[serde(default)]
    pub content: String,
    /// When the listing first went live. Present on most boards; this is the
    /// field that actually means "posted".
    #[serde(default)]
    pub first_published: Option<String>,
    /// Always present. Falls back here, accepting that an edited listing looks
    /// newer than it is — still far better than treating every job as brand new.
    #[serde(default)]
    pub updated_at: Option<String>,
}

#[derive(Deserialize)]
pub struct Loc {
    pub name: String,
}

impl Client {
    pub fn new(http: reqwest::Client) -> Self {
        let base = std::env::var("RADAR_GREENHOUSE_BASE").unwrap_or_else(|_| DEFAULT_BASE.into());
        if base != DEFAULT_BASE {
            tracing::warn!(%base, "greenhouse base URL overridden (fixture mode?)");
        }
        Self { http, base }
    }

    /// Every job on one board, or a sentence explaining why not.
    ///
    /// The error is a `String` rather than a typed enum on purpose: every caller
    /// does the same thing with it — shows it on the dashboard next to the board
    /// it belongs to — and a wrong board token is not a condition any code can
    /// recover from, only a person can.
    pub async fn jobs(&self, board: &str) -> Result<Vec<Job>, String> {
        let url = format!("{}/{board}/jobs?content=true", self.base.trim_end_matches('/'));
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("unreachable — {}", brief(&e)))?;

        let status = resp.status();
        if !status.is_success() {
            let code = status.as_u16();
            // A wrong token 404s, and a 404 looks exactly like an empty board
            // unless something says otherwise. This is that something.
            return Err(if code == 404 {
                "HTTP 404 — no such board token".to_string()
            } else {
                format!("HTTP {code}{}", status_hint(code))
            });
        }

        let data: Resp = resp
            .json()
            .await
            .map_err(|e| format!("unreadable response — {}", brief(&e)))?;
        Ok(data.jobs)
    }
}
