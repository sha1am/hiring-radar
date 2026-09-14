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
    base: String,
    /// Set once the first deep walk has filled the window. See `pages_for`.
    backfilled: std::sync::atomic::AtomicBool,
}

/// Overridable so the pagination and parsing can be exercised against a fixture
/// server. There is no other way to test this source — it cannot be reached from
/// a sandbox, and testing it against the real API means risking the account.
const DEFAULT_BASE: &str = "https://www.linkedin.com/voyager/api/graphql";

impl Voyager {
    pub fn new(client: reqwest::Client, cfg: VoyagerCfg, user_agent: String) -> Self {
        let base = std::env::var("RADAR_VOYAGER_BASE").unwrap_or_else(|_| DEFAULT_BASE.into());
        if base != DEFAULT_BASE {
            tracing::warn!(%base, "voyager base URL overridden (fixture mode?)");
        }
        Self {
            client,
            cfg,
            user_agent,
            base,
            backfilled: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// How many pages this crawl is allowed.
    ///
    /// The deep walk exists to fill the window once. Repeating it every 90
    /// seconds would mean three hashtags times eight pages every cycle —
    /// roughly a thousand authenticated requests an hour against an API that
    /// bans accounts for far less. After the window is filled, only new posts
    /// matter, and new posts are on page one. So: deep once, shallow forever
    /// after.
    fn pages_for(&self, live: &Settings) -> u32 {
        if self.backfilled.load(std::sync::atomic::Ordering::Relaxed) {
            live.voyager_poll_pages.max(1)
        } else {
            live.voyager_max_pages.max(1)
        }
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

        let cutoff = crate::model::now() - live.lookback_hours * 3600;
        let allowed_pages = self.pages_for(live);
        let deep = !self.backfilled.load(std::sync::atomic::Ordering::Relaxed);
        let mut posts: Vec<RawPost> = Vec::new();

        // One search per term. Hashtag searches are what surface "we're hiring"
        // posts; a single OR'd blob returns a worse mix than separate passes.
        for term in &live.voyager_queries {
            let mut term_kept = 0usize;
            let mut pages = 0u32;
            let mut oldest_seen: Option<i64> = None;
            let mut exhausted = false;

            // Walk back page by page until the window is covered. A single page
            // is whatever LinkedIn feels like returning — often under an hour on
            // a busy hashtag — so without this, "everything from the last 24
            // hours" quietly means "the most recent twenty posts".
            for page in 0..allowed_pages {
                pages = page + 1;
                let start_at = page as usize * PAGE_SIZE;

                // Sorted by date so walking pages walks backwards in time; under
                // relevance ordering there is no point at which it is safe to stop.
                let variables = format!(
                    "(start:{start_at},count:{PAGE_SIZE},query:(keywords:{},flagshipSearchIntent:SEARCH_CONTENT,sortBy:\"date_posted\"))",
                    urlish(term)
                );
                let url = format!(
                    "{}?queryId={}&variables={}",
                    self.base, live.voyager_query_id, variables
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
                        tracing::warn!(term, status = code, page, "voyager non-200");
                        out.fail(format!(
                            "{term}: HTTP {code}{}",
                            match code {
                                401 | 403 => " — cookie expired or rejected, refresh li_at",
                                429 => " — rate limited, widen the poll interval or lower max pages",
                                400 => " — queryId or variables rejected, the shape changed",
                                _ => "",
                            }
                        ));
                        exhausted = true;
                        break;
                    }
                    Err(e) => {
                        tracing::warn!(term, %e, page, "voyager fetch failed");
                        out.fail(format!("{term}: unreachable — {}", brief(&e)));
                        exhausted = true;
                        break;
                    }
                };

                let mut batch: Vec<RawPost> = Vec::new();
                walk(&body, &mut batch);

                if batch.is_empty() {
                    if page == 0 {
                        // A 200 that yields nothing on the FIRST page means the
                        // JSON shape moved, which is the likeliest way this
                        // source breaks. Report what the payload actually looked
                        // like so looks_like_post can be corrected without
                        // guessing. On a later page it just means we ran out.
                        out.fail(format!(
                            "{term}: HTTP 200 but no posts extracted — {}",
                            shape(&body)
                        ));
                    }
                    exhausted = true;
                    break;
                }

                // Keep only what is inside the window; note the oldest thing on
                // this page to decide whether to keep walking.
                let mut page_oldest = i64::MAX;
                for p in batch {
                    let ts = p.posted_at.unwrap_or_else(crate::model::now);
                    page_oldest = page_oldest.min(ts);
                    if ts >= cutoff {
                        posts.push(p);
                        term_kept += 1;
                    }
                }
                oldest_seen = Some(match oldest_seen {
                    Some(o) => o.min(page_oldest),
                    None => page_oldest,
                });

                // Past the window — everything further back is older still.
                if page_oldest < cutoff {
                    exhausted = true;
                    break;
                }

                // Be gentle. Pagination multiplies request volume against an
                // API that bans accounts, so pace it rather than hammering.
                tokio::time::sleep(std::time::Duration::from_millis(PAGE_DELAY_MS)).await;
            }

            if term_kept > 0 || !out.notes.iter().any(|n| n.text.starts_with(term.as_str())) {
                let reach = match oldest_seen {
                    Some(o) => {
                        let hrs = ((crate::model::now() - o) as f64 / 3600.0).max(0.0);
                        format!(", reached {hrs:.1}h back")
                    }
                    None => String::new(),
                };
                out.ok(format!(
                    "{term}: {term_kept} posts in last {}h ({pages} page{}{reach}){}",
                    live.lookback_hours,
                    if pages == 1 { "" } else { "s" },
                    if exhausted { "" } else { ", page limit hit" },
                ));
            }
        }

        // Only mark the window filled if something actually came back — a run
        // that failed on every term must not downgrade the next one to a
        // single page and quietly leave the window half-empty forever.
        if deep && !posts.is_empty() {
            self.backfilled
                .store(true, std::sync::atomic::Ordering::Relaxed);
            tracing::info!(
                posts = posts.len(),
                "voyager window filled; later crawls poll shallow"
            );
        }

        tracing::info!(found = posts.len(), deep, "voyager content search");
        out.posts = posts;
        Ok(out)
    }
}

/// Results per request. LinkedIn caps this; 20 is the web app's own value.
const PAGE_SIZE: usize = 20;
/// Pause between pages of the same search.
const PAGE_DELAY_MS: u64 = 700;

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

    let external_id_for_time = urn.clone();
    Some(RawPost {
        source: "linkedin_voyager".into(),
        external_id: urn,
        // A hiring post rarely has a clean title; use the first line as a proxy.
        title: text.lines().next().unwrap_or("Hiring post").chars().take(120).collect(),
        company: author,
        location: None,
        body: text.to_string(),
        // Voyager reports no timestamp; the activity id carries one.
        posted_at: crate::timeparse::from_linkedin_urn(&external_id_for_time),
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
