use super::{brief, Fetched, JobSource};
use crate::config::Voyager as VoyagerCfg;
use crate::model::{ApplyChannel, RawPost};
use crate::settings::Settings;
use serde_json::Value;

/// Authenticated content search against LinkedIn's internal Voyager API — the
/// same calls the web app makes. This is what surfaces personal "we're hiring!"
/// posts (as opposed to formal job listings).
///
/// EXPECT MAINTENANCE. The `query_id` and the shape of the response change when
/// LinkedIn ships. Treat the JSON walk below as a starting point: open DevTools
/// -> Network on a real content search, copy the current queryId into config,
/// and adjust `looks_like_post` to match the fields you see. Disabled unless
/// enabled=true in config AND the LI_AT cookie is present in the environment.
pub struct Voyager {
    client: reqwest::Client,
    cfg: VoyagerCfg,
    user_agent: String,
}

impl Voyager {
    pub fn new(client: reqwest::Client, cfg: VoyagerCfg, user_agent: String) -> Self {
        Self { client, cfg, user_agent }
    }

    /// Credentials and the queryId still come from config/env — they are
    /// secrets and a fragile constant, not things to edit in a web form. The
    /// dashboard toggle only decides whether to use them.
    /// Credentials stay in env — they are secrets. The queryId comes from
    /// settings, because it is a public constant that changes whenever
    /// LinkedIn ships and must be replaceable without a rebuild.
    fn missing(&self, live: &Settings) -> Option<&'static str> {
        if self.cfg.li_at.is_none() {
            return Some("LI_AT is not set — put your li_at cookie in .env and restart");
        }
        let qid = &live.voyager_query_id;
        if qid.is_empty() || qid.contains("REPLACE_ME") {
            return Some(
                "no queryId — grab one from DevTools → Network on a LinkedIn \
                 content search and paste it in Settings → Sources",
            );
        }
        if live.voyager_queries.is_empty() {
            return Some("no search terms — add #hiring in Settings → Sources");
        }
        None
    }
}

#[async_trait::async_trait]
impl JobSource for Voyager {
    fn is_enabled(&self, cfg: &Settings) -> bool {
        cfg.voyager_enabled
    }

    fn name(&self) -> &str {
        "linkedin_voyager"
    }

    async fn fetch(&self, live: &Settings) -> anyhow::Result<Fetched> {
        let mut out = Fetched::default();
        if !live.voyager_enabled {
            return Ok(out);
        }
        if let Some(why) = self.missing(live) {
            out.fail(why);
            tracing::debug!(why, "voyager not configured; skipping");
            return Ok(out);
        }
        let li_at = self.cfg.li_at.as_deref().unwrap_or_default();
        let jsession = self.cfg.jsessionid.as_deref().unwrap_or("ajax:0000000000000000000");
        // LinkedIn's CSRF token is the JSESSIONID value verbatim.
        let csrf = jsession.trim_matches('"');
        let cookie = format!("li_at={li_at}; JSESSIONID=\"{jsession}\"");

        let mut posts = Vec::new();

        // One search per term. Hashtag searches are what surface "we're hiring"
        // posts; a single OR'd blob returns a worse mix than separate passes.
        for term in &live.voyager_queries {
            let variables = format!(
                "(query:(keywords:{},flagshipSearchIntent:SEARCH_CONTENT))",
                urlish(term)
            );
            let url = format!(
                "https://www.linkedin.com/voyager/api/graphql?queryId={}&variables={}",
                live.voyager_query_id, variables
            );

            let resp = self
                .client
                .get(&url)
                .header("cookie", &cookie)
                .header("csrf-token", csrf)
                .header("accept", "application/vnd.linkedin.normalized+json+2.1")
                .header("x-restli-protocol-version", "2.0.0")
                .header("x-li-lang", "en_US")
                .header("user-agent", &self.user_agent)
                .header("referer", "https://www.linkedin.com/search/results/content/")
                .send()
                .await;

            let body: Value = match resp {
                Ok(r) if r.status().is_success() => r.json().await.unwrap_or(Value::Null),
                Ok(r) => {
                    let code = r.status().as_u16();
                    tracing::warn!(term, status = code, "voyager non-200");
                    out.fail(format!(
                        "{term}: HTTP {code}{}",
                        match code {
                            401 | 403 => " — cookie expired or rejected, refresh li_at",
                            429 => " — rate limited, widen the poll interval",
                            400 => " — queryId or variables rejected, the shape changed",
                            _ => "",
                        }
                    ));
                    continue;
                }
                Err(e) => {
                    tracing::warn!(term, %e, "voyager fetch failed");
                    out.fail(format!("{term}: unreachable — {}", brief(&e)));
                    continue;
                }
            };

            let before = posts.len();
            walk(&body, &mut posts);
            let found = posts.len() - before;

            if found == 0 {
                // A 200 that yields nothing means the JSON shape moved, which is
                // the single most likely way this source breaks. Report what the
                // payload actually looked like so the field names in
                // looks_like_post can be corrected without guessing.
                out.fail(format!("{term}: HTTP 200 but no posts extracted — {}", shape(&body)));
            } else {
                out.ok(format!("{term}: {found} posts"));
            }
        }

        tracing::info!(found = posts.len(), "voyager content search");
        out.posts = posts;
        Ok(out)
    }
}

/// A one-line description of an unexpected payload: top-level keys and the size
/// of the usual containers. Enough to tell "auth wall" from "renamed fields".
fn shape(v: &Value) -> String {
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
        other => format!("unexpected root: {}", other),
    }
}



/// Recursively scan the normalised JSON for anything that looks like a feed post.
fn walk(v: &Value, out: &mut Vec<RawPost>) {
    match v {
        Value::Object(map) => {
            if let Some(post) = looks_like_post(map) {
                out.push(post);
            }
            for val in map.values() {
                walk(val, out);
            }
        }
        Value::Array(arr) => {
            for val in arr {
                walk(val, out);
            }
        }
        _ => {}
    }
}

/// Heuristic extractor. Adjust the field names to whatever the live payload uses.
fn looks_like_post(map: &serde_json::Map<String, Value>) -> Option<RawPost> {
    // Commentary text lives under a few possible paths across API versions.
    let text = map
        .get("commentary")
        .and_then(|c| c.get("text"))
        .and_then(|t| t.get("text"))
        .and_then(|t| t.as_str())
        .or_else(|| map.get("commentaryText").and_then(|t| t.as_str()))?;

    if text.trim().is_empty() {
        return None;
    }

    let urn = map
        .get("entityUrn")
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();
    if urn.is_empty() {
        return None;
    }

    let author = map
        .get("actor")
        .and_then(|a| a.get("name"))
        .and_then(|n| n.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or("LinkedIn member")
        .to_string();

    let profile_url = map
        .get("actor")
        .and_then(|a| a.get("navigationContext"))
        .and_then(|n| n.get("actionTarget"))
        .and_then(|t| t.as_str())
        .map(|s| s.to_string());

    let apply = match extract_email(text) {
        Some(email) => ApplyChannel::Email(email),
        None => match profile_url.clone() {
            Some(p) => ApplyChannel::LinkedInDm { profile_url: p },
            None => ApplyChannel::Unknown,
        },
    };

    let url = profile_url.unwrap_or_else(|| format!("https://www.linkedin.com/feed/update/{urn}"));

    Some(RawPost {
        source: "linkedin_voyager".into(),
        external_id: urn,
        // A hiring post rarely has a clean title; use the first line as a proxy.
        title: text.lines().next().unwrap_or("Hiring post").chars().take(120).collect(),
        company: author,
        location: None,
        body: text.to_string(),
        posted_at: None,
        url,
        apply,
        // A feed post has no title; this is its first line.
        synthetic_title: true,
    })
}

fn extract_email(text: &str) -> Option<String> {
    text.split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '@' && c != '.'))
        .find(|w| {
            let at = w.find('@');
            match at {
                Some(i) => i > 0 && w[i + 1..].contains('.') && !w.ends_with('.'),
                None => false,
            }
        })
        .map(|w| w.to_string())
}

/// Minimal encoding for the Voyager variables blob (which is not standard urlencoding).
fn urlish(s: &str) -> String {
    s.replace(' ', "%20").replace(',', "%2C").replace('(', "%28").replace(')', "%29")
}
