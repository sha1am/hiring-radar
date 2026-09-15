//! The Workday crawl strategy.

use super::client::{Client, PAGE_SIZE};
use super::parse;
use super::target::{self, Target};
use crate::model::RawPost;
use crate::settings::Settings;
use crate::sources::common::companies;
use crate::sources::{Fetched, JobSource};
use std::collections::HashSet;
use std::sync::Mutex;

/// Pages per tenant per crawl. 10 pages is 200 openings, which covers all but
/// the largest employers; the cap exists so one company with four thousand
/// listings cannot eat an entire cycle.
const MAX_PAGES: usize = 10;

/// Pause between pages of the same tenant. Workday is public and won't ban you,
/// but it is somebody's careers site, not a load target.
const PAGE_DELAY_MS: u64 = 250;

/// Descriptions to fetch per tenant per crawl.
///
/// The listing has no description, and the description is most of what the
/// resume scorer has to work with — a Workday job scored on its title alone
/// competes badly against a Greenhouse job scored on two pages of detail. But
/// each one is its own request, so they are rationed, and `detailed` below means
/// the budget is spent on postings we have never described rather than
/// re-fetching the same eight every cycle.
const DETAIL_BUDGET: usize = 8;

pub struct Workday {
    client: Client,
    /// Postings whose description we already fetched, this process. Bounded, and
    /// lost on restart, which costs one extra pass — acceptable for not needing
    /// database access inside a source.
    detailed: Mutex<HashSet<String>>,
}

/// Stop the memo growing without bound on a long-running instance.
const MEMO_CAP: usize = 20_000;

impl Workday {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            client: Client::new(http),
            detailed: Mutex::new(HashSet::new()),
        }
    }

    fn already_detailed(&self, key: &str) -> bool {
        self.detailed
            .lock()
            .map(|m| m.contains(key))
            .unwrap_or(false)
    }

    fn remember_detailed(&self, key: String) {
        if let Ok(mut m) = self.detailed.lock() {
            if m.len() >= MEMO_CAP {
                m.clear();
            }
            m.insert(key);
        }
    }
}

#[async_trait::async_trait]
impl JobSource for Workday {
    fn name(&self) -> &str {
        parse::SOURCE
    }

    fn is_enabled(&self, s: &Settings) -> bool {
        s.workday_enabled
    }

    async fn fetch(&self, s: &Settings) -> anyhow::Result<Fetched> {
        let mut out = Fetched::default();
        if !s.workday_enabled {
            return Ok(out);
        }

        let list = companies::load("workday");
        out.info(list.note());
        let entries = companies::resolve(&list, &s.workday_sites);
        if entries.is_empty() {
            out.info(format!(
                "nothing to crawl — add careers URLs to {}",
                list.path.display()
            ));
            return Ok(out);
        }

        let now = crate::model::now();
        let cutoff = now - s.workday_lookback_days.max(1) * 86_400;

        for entry in &entries {
            let t = match target::parse(entry) {
                Ok(t) => t,
                Err(why) => {
                    // A typo'd tenant is indistinguishable from a company with
                    // no openings unless we say so here.
                    out.fail(format!("{entry}: {why}"));
                    continue;
                }
            };
            self.crawl_target(&t, cutoff, now, &mut out).await;
        }

        tracing::info!(found = out.posts.len(), tenants = entries.len(), "workday crawl");
        Ok(out)
    }
}

impl Workday {
    async fn crawl_target(&self, t: &Target, cutoff: i64, now: i64, out: &mut Fetched) {
        let company = parse::company_for(t);
        let mut fresh: Vec<RawPost> = Vec::new();
        let mut seen_total = 0usize;
        let mut pages = 0usize;
        let mut budget = DETAIL_BUDGET;
        let mut truncated = true;

        'pages: for page in 0..MAX_PAGES {
            pages = page + 1;
            let batch = match self.client.page(t, page * PAGE_SIZE).await {
                Ok(p) => p,
                Err(why) => {
                    tracing::warn!(target = %t.label(), why, page, "workday page failed");
                    out.fail(format!("{}: {why}", t.label()));
                    return;
                }
            };

            let short = batch.job_postings.len() < PAGE_SIZE;
            seen_total += batch.job_postings.len();

            for p in batch.job_postings {
                // Cheap pass first: the display string is free, the description
                // is a request. Anything outside the window never costs one.
                let when = p
                    .posted_on
                    .as_deref()
                    .and_then(|d| parse::posted_at(d, now));
                if when.is_some_and(|w| w < cutoff) {
                    continue;
                }

                // No path means nothing to fetch and nothing to link to; the
                // parser drops these, so don't spend a request on one first.
                let Some(path) = p.external_path.clone() else {
                    continue;
                };
                let key = format!("{}:{path}", t.tenant);
                let detail = if budget > 0 && !self.already_detailed(&key) {
                    match self.client.detail(t, &path).await {
                        Ok(d) => {
                            budget -= 1;
                            self.remember_detailed(key);
                            Some(d)
                        }
                        Err(why) => {
                            // One unreadable job is not a broken tenant; keep the
                            // card, lose the description.
                            tracing::debug!(target = %t.label(), why, "workday detail failed");
                            budget -= 1;
                            None
                        }
                    }
                } else {
                    None
                };

                if let Some(post) = parse::to_post(t, &company, p, detail, now) {
                    fresh.push(post);
                }
            }

            // `total` is 0 from page two onward, so it cannot end this loop. A
            // short page is the only honest signal that there is no more.
            if short {
                truncated = false;
                break 'pages;
            }
            tokio::time::sleep(std::time::Duration::from_millis(PAGE_DELAY_MS)).await;
        }

        out.ok(format!(
            "{}: {} in last {}d of {seen_total} listed ({pages} page{}{})",
            t.label(),
            fresh.len(),
            (crate::model::now() - cutoff) / 86_400,
            if pages == 1 { "" } else { "s" },
            if truncated { ", page limit hit" } else { "" },
        ));
        out.posts.extend(fresh);
    }
}
