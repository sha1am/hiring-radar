//! The Voyager crawl strategy: deep once, shallow forever after.

use super::client::{shape, Client};
use super::parse;
use crate::config::Voyager as VoyagerCfg;
use crate::model::RawPost;
use crate::settings::Settings;
use crate::sources::linkedin::auth;
use crate::sources::{Fetched, JobSource};
use std::sync::atomic::{AtomicBool, Ordering};

/// Pause between pages of the same search.
const PAGE_DELAY_MS: u64 = 700;

pub struct Voyager {
    client: Client,
    cfg: VoyagerCfg,
    /// Set once the first deep walk has filled the window. See `pages_for`.
    backfilled: AtomicBool,
}

impl Voyager {
    pub fn new(http: reqwest::Client, cfg: VoyagerCfg, user_agent: String) -> Self {
        Self {
            client: Client::new(http, user_agent),
            cfg,
            backfilled: AtomicBool::new(false),
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
        if self.backfilled.load(Ordering::Relaxed) {
            live.voyager_poll_pages.max(1)
        } else {
            live.voyager_max_pages.max(1)
        }
    }

    /// Credentials stay in env — they are secrets. The queryId comes from
    /// settings, because it is a public constant that changes whenever LinkedIn
    /// ships and must be replaceable without a rebuild.
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

/// What one search term's walk produced.
struct TermWalk {
    kept: usize,
    pages: u32,
    oldest_seen: Option<i64>,
    /// We stopped because we ran out of results or ran past the window, rather
    /// than because we hit the page ceiling. The difference matters: hitting the
    /// ceiling means the window is *not* fully covered.
    exhausted: bool,
    failed: bool,
}

#[async_trait::async_trait]
impl JobSource for Voyager {
    fn name(&self) -> &str {
        parse::SOURCE
    }

    fn is_enabled(&self, s: &Settings) -> bool {
        s.voyager_enabled
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

        let session = auth::session(
            self.cfg.li_at.as_deref().unwrap_or_default(),
            self.cfg.jsessionid.as_deref(),
        );
        let cutoff = crate::model::now() - live.lookback_hours * 3600;
        let allowed_pages = self.pages_for(live);
        let deep = !self.backfilled.load(Ordering::Relaxed);
        let mut posts: Vec<RawPost> = Vec::new();

        // One search per term. Hashtag searches are what surface "we're hiring"
        // posts; a single OR'd blob returns a worse mix than separate passes.
        for term in &live.voyager_queries {
            let walk = self
                .walk_term(
                    &session,
                    live,
                    term,
                    allowed_pages,
                    cutoff,
                    &mut posts,
                    &mut out,
                )
                .await;

            if walk.failed && walk.kept == 0 {
                continue; // the failure note already says what happened
            }
            let reach = match walk.oldest_seen {
                Some(o) => {
                    let hrs = ((crate::model::now() - o) as f64 / 3600.0).max(0.0);
                    format!(", reached {hrs:.1}h back")
                }
                None => String::new(),
            };
            out.ok(format!(
                "{term}: {} posts in last {}h ({} page{}{reach}){}",
                walk.kept,
                live.lookback_hours,
                walk.pages,
                if walk.pages == 1 { "" } else { "s" },
                if walk.exhausted {
                    ""
                } else {
                    ", page limit hit"
                },
            ));
        }

        // Only mark the window filled if something actually came back — a run
        // that failed on every term must not downgrade the next one to a single
        // page and quietly leave the window half-empty forever.
        if deep && !posts.is_empty() {
            self.backfilled.store(true, Ordering::Relaxed);
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

impl Voyager {
    /// Walk one search term back through pages until the window is covered.
    ///
    /// Without this, "everything from the last 24 hours" quietly means "the most
    /// recent twenty posts" — a single page is whatever LinkedIn feels like
    /// returning, often under an hour on a busy hashtag.
    #[allow(clippy::too_many_arguments)]
    async fn walk_term(
        &self,
        session: &auth::Session,
        live: &Settings,
        term: &str,
        allowed_pages: u32,
        cutoff: i64,
        posts: &mut Vec<RawPost>,
        out: &mut Fetched,
    ) -> TermWalk {
        let mut w = TermWalk {
            kept: 0,
            pages: 0,
            oldest_seen: None,
            exhausted: false,
            failed: false,
        };

        for page in 0..allowed_pages {
            w.pages = page + 1;
            let body = match self
                .client
                .page(session, &live.voyager_query_id, term, page)
                .await
            {
                Ok(b) => b,
                Err(why) => {
                    tracing::warn!(term, why, page, "voyager page failed");
                    out.fail(format!("{term}: {why}"));
                    w.failed = true;
                    w.exhausted = true;
                    return w;
                }
            };

            let batch = parse::posts(&body);
            if batch.is_empty() {
                if page == 0 {
                    // A 200 that yields nothing on the FIRST page means the JSON
                    // shape moved, which is the likeliest way this source breaks.
                    // Report what the payload actually looked like so the
                    // extractor can be corrected without guessing. On a later
                    // page it just means we ran out.
                    out.fail(format!(
                        "{term}: HTTP 200 but no posts extracted — {}",
                        shape(&body)
                    ));
                    w.failed = true;
                }
                w.exhausted = true;
                return w;
            }

            // Keep only what is inside the window; note the oldest thing on this
            // page to decide whether to keep walking.
            let mut page_oldest = i64::MAX;
            for p in batch {
                let ts = p.posted_at.unwrap_or_else(crate::model::now);
                page_oldest = page_oldest.min(ts);
                if ts >= cutoff {
                    posts.push(p);
                    w.kept += 1;
                }
            }
            w.oldest_seen = Some(match w.oldest_seen {
                Some(o) => o.min(page_oldest),
                None => page_oldest,
            });

            // Past the window — everything further back is older still.
            if page_oldest < cutoff {
                w.exhausted = true;
                return w;
            }

            // Be gentle. Pagination multiplies request volume against an API
            // that bans accounts, so pace it rather than hammering.
            tokio::time::sleep(std::time::Duration::from_millis(PAGE_DELAY_MS)).await;
        }
        w
    }
}
