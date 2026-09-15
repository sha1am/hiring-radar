//! The guest endpoint: HTML job cards, no auth.

use crate::sources::common::{brief, status_hint};

/// The public "see more job postings" endpoint. `f_TPR=r3600` asks for postings
/// from the last hour, which is exactly the freshness window this tool cares
/// about.
const URL: &str = "https://www.linkedin.com/jobs-guest/jobs/api/seeMoreJobPostings/search";

pub struct Client {
    http: reqwest::Client,
    base: String,
}

impl Client {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            http,
            base: std::env::var("RADAR_LI_GUEST_BASE").unwrap_or_else(|_| URL.into()),
        }
    }

    /// The raw HTML fragment for one search, or why it didn't arrive.
    pub async fn search(&self, keywords: &str, location: &str) -> Result<String, String> {
        let resp = self
            .http
            .get(&self.base)
            .query(&[
                ("keywords", keywords),
                ("location", location),
                ("f_TPR", "r3600"),
                ("start", "0"),
            ])
            .send()
            .await
            .map_err(|e| format!("unreachable — {}", brief(&e)))?;

        let status = resp.status();
        if !status.is_success() {
            let code = status.as_u16();
            return Err(format!("HTTP {code}{}", status_hint(code)));
        }
        resp.text()
            .await
            .map_err(|e| format!("unreadable response — {}", brief(&e)))
    }
}
