//! The Workday CXS API, and the two undocumented traps in it.

use super::target::Target;
use crate::sources::common::{brief, status_hint};
use serde::Deserialize;

/// Postings per request.
///
/// **Do not raise this.** The API documents no limit and silently caps it at 20:
/// ask for 50 and you get either an HTTP 400 or, worse, a 200 with zero rows,
/// which reads exactly like a company with no openings. Paging is the only way
/// to go deeper.
pub const PAGE_SIZE: usize = 20;

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct JobsPage {
    #[serde(default)]
    pub job_postings: Vec<Posting>,
    /// **Not usable for pagination.** It is correct on the first page and then
    /// collapses to 0 on every page after, so the obvious
    /// `while fetched < total` loop exits after one page and quietly reports the
    /// first twenty jobs as the whole company. Page until a page comes back
    /// short instead.
    #[serde(default)]
    pub total: i64,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Posting {
    pub title: String,
    /// Path relative to the site root, e.g. `/job/Bengaluru/Engineer_R12345`.
    pub external_path: String,
    #[serde(default)]
    pub locations_text: Option<String>,
    /// A localised display string — "Posted Today", "Posted 30+ Days Ago". Not
    /// a date, and not parseable as one.
    #[serde(default)]
    pub posted_on: Option<String>,
    /// Usually the requisition id. Present on most sites, absent on some.
    #[serde(default)]
    pub bullet_fields: Vec<String>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Detail {
    #[serde(default)]
    pub job_description: Option<String>,
    /// An actual date, unlike `posted_on` — but it costs one request per job.
    #[serde(default)]
    pub start_date: Option<String>,
    #[serde(default)]
    pub job_req_id: Option<String>,
    #[serde(default)]
    pub country: Option<Country>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Country {
    #[serde(default)]
    pub descriptor: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
struct DetailEnvelope {
    #[serde(rename = "jobPostingInfo", default)]
    job_posting_info: Option<Detail>,
}

pub struct Client {
    http: reqwest::Client,
    /// Overridable so pagination and parsing can be exercised against a fixture
    /// server. Workday is per-tenant, so this replaces the whole host.
    base_override: Option<String>,
}

impl Client {
    pub fn new(http: reqwest::Client) -> Self {
        let base_override = std::env::var("RADAR_WORKDAY_BASE").ok();
        if let Some(b) = &base_override {
            tracing::warn!(base = %b, "workday base URL overridden (fixture mode?)");
        }
        Self {
            http,
            base_override,
        }
    }

    fn api_root(&self, t: &Target) -> String {
        match &self.base_override {
            // The fixture keeps the tenant/site shape so URL construction is
            // still what gets exercised, not bypassed.
            Some(b) => format!(
                "{}/wday/cxs/{}/{}",
                b.trim_end_matches('/'),
                t.tenant,
                t.site
            ),
            None => t.api(),
        }
    }

    /// One page of postings.
    pub async fn page(&self, t: &Target, offset: usize) -> Result<JobsPage, String> {
        let url = format!("{}/jobs", self.api_root(t));
        let body = serde_json::json!({
            "appliedFacets": {},
            "limit": PAGE_SIZE,
            "offset": offset,
            "searchText": ""
        });

        let resp = self
            .http
            .post(&url)
            .header("content-type", "application/json")
            .header("accept", "application/json")
            // Without this some tenants return a localised body, or a 415.
            .header("accept-language", "en-US")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("unreachable — {}", brief(&e)))?;

        let status = resp.status();
        if !status.is_success() {
            let code = status.as_u16();
            return Err(match code {
                404 => "HTTP 404 — no such tenant or site; check the careers URL".to_string(),
                _ => format!("HTTP {code}{}", status_hint(code)),
            });
        }

        resp.json::<JobsPage>()
            .await
            .map_err(|e| format!("unreadable response — {}", brief(&e)))
    }

    /// One posting's description. Costs a request, so callers ration it.
    pub async fn detail(&self, t: &Target, external_path: &str) -> Result<Detail, String> {
        let url = format!(
            "{}{}",
            self.api_root(t),
            if external_path.starts_with('/') {
                external_path.to_string()
            } else {
                format!("/{external_path}")
            }
        );
        let resp = self
            .http
            .get(&url)
            .header("accept", "application/json")
            .header("accept-language", "en-US")
            .send()
            .await
            .map_err(|e| format!("unreachable — {}", brief(&e)))?;

        if !resp.status().is_success() {
            let code = resp.status().as_u16();
            return Err(format!("HTTP {code}{}", status_hint(code)));
        }

        let env: DetailEnvelope = resp
            .json()
            .await
            .map_err(|e| format!("unreadable response — {}", brief(&e)))?;
        Ok(env.job_posting_info.unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_size_stays_at_the_silent_cap() {
        // Raising this is the bug that returns zero jobs and looks like a
        // company with no openings. The test exists to make that a red build
        // rather than a quiet evening.
        assert_eq!(PAGE_SIZE, 20);
    }

    #[test]
    fn a_page_two_payload_with_total_zero_still_carries_jobs() {
        // This is the real shape of page two: total collapses, postings remain.
        let p: JobsPage = serde_json::from_value(serde_json::json!({
            "total": 0,
            "jobPostings": [{
                "title": "Backend Engineer",
                "externalPath": "/job/Bengaluru/Backend_R1",
                "locationsText": "Bengaluru, India",
                "postedOn": "Posted Today",
                "bulletFields": ["R1"]
            }]
        }))
        .unwrap();
        assert_eq!(p.total, 0);
        assert_eq!(p.job_postings.len(), 1);
    }

    #[test]
    fn a_posting_missing_the_optional_fields_still_parses() {
        // Several tenants omit bulletFields and postedOn entirely.
        let p: JobsPage = serde_json::from_value(serde_json::json!({
            "jobPostings": [{"title": "SRE", "externalPath": "/job/X"}]
        }))
        .unwrap();
        assert_eq!(p.job_postings[0].title, "SRE");
        assert!(p.job_postings[0].posted_on.is_none());
    }

    #[test]
    fn the_detail_envelope_is_unwrapped() {
        let d: DetailEnvelope = serde_json::from_value(serde_json::json!({
            "jobPostingInfo": {
                "jobDescription": "<p>Go</p>",
                "startDate": "2026-09-01",
                "jobReqId": "R1",
                "country": {"descriptor": "India"}
            }
        }))
        .unwrap();
        let d = d.job_posting_info.unwrap();
        assert_eq!(d.job_req_id.as_deref(), Some("R1"));
        assert_eq!(d.country.unwrap().descriptor.as_deref(), Some("India"));
    }
}
