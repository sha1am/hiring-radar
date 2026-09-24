//! The Voyager GraphQL call, and the vocabulary for describing how it failed.

use crate::sources::common::brief;
use crate::sources::linkedin::auth::Session;
use serde_json::Value;

/// Overridable so the pagination and parsing can be exercised against a fixture
/// server. There is no other way to test this source — it cannot be reached
/// from a sandbox, and testing it against the real API means risking the account.
pub const DEFAULT_BASE: &str = "https://www.linkedin.com/voyager/api/graphql";

/// Results per request. LinkedIn caps this; 20 is the web app's own value.
pub const PAGE_SIZE: usize = 20;

pub struct Client {
    http: reqwest::Client,
    base: String,
    user_agent: String,
}

impl Client {
    pub fn new(http: reqwest::Client, user_agent: String) -> Self {
        let base = std::env::var("RADAR_VOYAGER_BASE").unwrap_or_else(|_| DEFAULT_BASE.into());
        if base != DEFAULT_BASE {
            tracing::warn!(%base, "voyager base URL overridden (fixture mode?)");
        }
        Self {
            http,
            base,
            user_agent,
        }
    }

    /// One page of a content search, sorted newest first.
    pub async fn page(
        &self,
        session: &Session,
        query_id: &str,
        term: &str,
        page: u32,
    ) -> Result<Value, String> {
        let url = format!(
            "{}?queryId={query_id}&variables={}",
            self.base,
            variables(term, page as usize * PAGE_SIZE)
        );

        let resp = self
            .http
            .get(&url)
            .header("cookie", &session.cookie)
            .header("csrf-token", &session.csrf)
            .header("accept", "application/vnd.linkedin.normalized+json+2.1")
            .header("x-restli-protocol-version", "2.0.0")
            .header("x-li-lang", "en_US")
            .header("user-agent", &self.user_agent)
            .header(
                "referer",
                "https://www.linkedin.com/search/results/content/",
            )
            .send()
            .await
            .map_err(|e| format!("unreachable — {}", brief(&e)))?;

        let status = resp.status();
        if !status.is_success() {
            let code = status.as_u16();
            // Deliberately more specific than the shared status_hint: for this
            // source a 403 means your cookie, and a 400 means the queryId, and
            // saying so is the difference between a five-minute fix and an
            // evening of guessing.
            return Err(format!(
                "HTTP {code}{}",
                match code {
                    401 | 403 => " — cookie expired or rejected, refresh li_at",
                    429 => " — rate limited, widen the poll interval or lower max pages",
                    400 => " — queryId or variables rejected, the shape changed",
                    _ => "",
                }
            ));
        }

        Ok(resp.json().await.unwrap_or(Value::Null))
    }
}

/// The `variables` blob, which is LinkedIn's own bracket syntax rather than
/// JSON or standard query encoding.
///
/// Sorted by date so walking pages walks backwards in time. Under relevance
/// ordering there is no point at which it is safe to stop paginating, which
/// makes a bounded 24-hour window impossible.
pub fn variables(term: &str, start_at: usize) -> String {
    format!(
        "(start:{start_at},count:{PAGE_SIZE},query:(keywords:{},flagshipSearchIntent:SEARCH_CONTENT,sortBy:\"date_posted\"))",
        urlish(term)
    )
}

/// Minimal encoding for the variables blob, which is not standard urlencoding —
/// its own parens and commas are structural and must survive.
fn urlish(s: &str) -> String {
    s.replace(' ', "%20")
        .replace(',', "%2C")
        .replace('(', "%28")
        .replace(')', "%29")
}

/// A one-line description of an unexpected payload: top-level keys and the size
/// of the usual containers. Enough to tell "auth wall" from "renamed fields".
pub fn shape(v: &Value) -> String {
    match v {
        Value::Object(m) => {
            let keys: Vec<String> = m.keys().take(6).cloned().collect();
            let included = m
                .get("included")
                .and_then(|i| i.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            format!("keys: [{}], included: {}", keys.join(", "), included)
        }
        Value::Null => "empty/unparseable response".into(),
        other => format!("unexpected root: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paging_advances_by_a_page_not_by_one() {
        assert!(variables("#hiring", 0).contains("start:0"));
        assert!(variables("#hiring", 2 * PAGE_SIZE).contains("start:40"));
    }

    #[test]
    fn structural_punctuation_survives_encoding() {
        // The parens around the blob are syntax; the ones inside a search term
        // are data. Encoding must not blur the two.
        let v = variables("we are hiring (backend)", 0);
        assert!(v.starts_with("(start:"), "{v}");
        assert!(v.contains("we%20are%20hiring%20%28backend%29"), "{v}");
    }

    #[test]
    fn date_sort_is_not_optional() {
        // Relevance ordering makes a bounded time window impossible to walk.
        assert!(variables("#hiring", 0).contains("sortBy:\"date_posted\""));
    }

    #[test]
    fn shape_names_what_came_back_instead() {
        let v: Value = serde_json::json!({"status": 999, "included": [1, 2]});
        let s = shape(&v);
        assert!(s.contains("status"), "{s}");
        assert!(s.contains("included: 2"), "{s}");
    }
}
