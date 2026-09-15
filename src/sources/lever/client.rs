//! The Lever postings API: one call, one wire format.

use crate::sources::common::{brief, status_hint};
use serde::Deserialize;

/// Overridable so the pipeline can be exercised against a fixture server.
pub const DEFAULT_BASE: &str = "https://api.lever.co/v0/postings";

pub struct Client {
    http: reqwest::Client,
    base: String,
}

/// Unlike Greenhouse, Lever returns a bare array rather than an envelope.
#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Posting {
    pub id: String,
    /// The job title. Lever calls it `text`.
    pub text: String,
    pub hosted_url: String,
    #[serde(default)]
    pub apply_url: Option<String>,
    /// Milliseconds since the epoch — not seconds, which is the easy mistake
    /// here and puts every listing fifty thousand years in the future.
    #[serde(default)]
    pub created_at: Option<i64>,
    #[serde(default)]
    pub categories: Categories,
    /// Plain text is provided, so no tag stripping is needed for this source.
    #[serde(default)]
    pub description_plain: String,
    /// The requirements/benefits blocks, which is where the stack usually is.
    #[serde(default)]
    pub lists: Vec<ListBlock>,
    #[serde(default)]
    pub additional_plain: String,
}

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Categories {
    #[serde(default)]
    pub commitment: Option<String>,
    #[serde(default)]
    pub department: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub team: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
pub struct ListBlock {
    #[serde(default)]
    pub text: String,
    /// HTML. The bullets here are where "5+ years of Go" usually lives, so it
    /// matters for both scoring and the years extractor.
    #[serde(default)]
    pub content: String,
}

impl Client {
    pub fn new(http: reqwest::Client) -> Self {
        let base = std::env::var("RADAR_LEVER_BASE").unwrap_or_else(|_| DEFAULT_BASE.into());
        if base != DEFAULT_BASE {
            tracing::warn!(%base, "lever base URL overridden (fixture mode?)");
        }
        Self { http, base }
    }

    pub async fn postings(&self, slug: &str) -> Result<Vec<Posting>, String> {
        let url = format!("{}/{slug}?mode=json", self.base.trim_end_matches('/'));
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("unreachable — {}", brief(&e)))?;

        let status = resp.status();
        if !status.is_success() {
            let code = status.as_u16();
            // Same silent failure as Greenhouse: a wrong slug 404s, and a 404
            // is indistinguishable from an empty board unless we say so.
            return Err(if code == 404 {
                "HTTP 404 — no such Lever board; check the slug in jobs.lever.co/<slug>".into()
            } else {
                format!("HTTP {code}{}", status_hint(code))
            });
        }

        resp.json::<Vec<Posting>>()
            .await
            .map_err(|e| {
                let msg = e.to_string();
                // "unreadable response" sent people looking for a schema change
                // that was never there: Stripe and GitLab publish hundreds of
                // jobs with full descriptions, and the body simply did not
                // finish arriving inside the timeout.
                if msg.contains("timed out") {
                    "timed out reading the response — large board, raise RADAR_HTTP_TIMEOUT".into()
                } else {
                    format!("unreadable response — {}", brief(&e))
                }
            })
    }
}
