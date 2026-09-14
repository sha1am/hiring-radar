use crate::model::now;
use crate::{db, notify};
use crate::state::AppState;
use std::collections::HashMap;

/// The heart of the system. Detection continuously fills a scored pool; this
/// function decides *who fires when a slot is free*, under a hard rolling-hour
/// budget. It runs on a short tick AND immediately after each crawl batch, so
/// exceptional posts go out within seconds while the cap is never exceeded.
///
/// It is idempotent and safe to call concurrently — DB `mark_fired` enforces
/// exactly-once via a UNIQUE constraint, so a race just no-ops the loser.
pub async fn run(state: &AppState) -> anyhow::Result<()> {
    let live = state.settings().await;
    let rel = live.as_ref();

    // 1. Retire anything that aged out without ever firing.
    let expired = db::expire_stale(&state.pool).await?;
    if expired > 0 {
        tracing::debug!(expired, "candidates expired");
    }

    // 2. How many of this hour's slots remain?
    let used = db::budget_used(&state.pool).await?;
    let mut remaining = rel.per_hour_cap - used;
    if remaining <= 0 {
        return Ok(()); // budget spent; everyone keeps rolling until next hour drains
    }

    // 3. Adaptive bar: strict at the top of the hour, relaxed as it drains, so a
    //    quiet hour doesn't waste the budget on nothing. Exceptional bypasses it.
    let bar = adaptive_bar(rel);

    // 4. Walk the eligible pool best-first; fire until the budget or the pool runs out.
    //    Ranking is recomputed here, not read from the stored column — see
    //    Candidate::live_priority for why.
    let mut pool = db::eligible(&state.pool).await?;
    pool.sort_by(|a, b| {
        b.live_priority()
            .partial_cmp(&a.live_priority())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    // Per-company tally for THIS pass, so per_poster_cap > 1 is actually
    // reachable. A HashSet here capped every company at one regardless of config.
    let mut chosen_companies: HashMap<String, i64> = HashMap::new();
    let mut fired_any = false;

    for cand in pool {
        if remaining <= 0 {
            break;
        }
        let is_exceptional = cand.tier == "exceptional";

        // Adaptive gate (exceptional always passes).
        if !is_exceptional && cand.score < bar {
            continue;
        }

        // Per-poster cap: the trailing hour AND this pass count against it.
        let poster_prev = db::poster_used(&state.pool, &cand.company).await?;
        let poster_pass = chosen_companies.get(&cand.company).copied().unwrap_or(0);
        if poster_prev + poster_pass >= rel.per_poster_cap {
            continue;
        }

        // Ensure a draft exists (strong+ were drafted at detection; this is a safety net).
        let mut cand = cand;
        if cand.draft_body.is_none() {
            let post = to_raw(&cand);
            let (subject, body) = state
                .drafter
                .draft(&post, rel, &state.cfg.profile.name, &state.cfg.profile.email)
                .await;
            db::set_draft(&state.pool, cand.id, &subject, &body).await?;
            cand.draft_subject = Some(subject);
            cand.draft_body = Some(body);
        }

        // Claim the slot atomically. If another tick already fired it, skip.
        if !db::mark_fired(&state.pool, &cand).await? {
            continue;
        }

        // Dispatch: push is the ping, email is the record. Failures don't unfire.
        if let Err(e) = notify::push_ntfy(&state.http, &state.cfg, &cand).await {
            tracing::warn!(id = cand.id, %e, "ntfy failed");
        }
        if let Err(e) = notify::email_self(&state.cfg, &cand).await {
            tracing::warn!(id = cand.id, %e, "self-email failed");
        }

        tracing::info!(
            id = cand.id, tier = %cand.tier, score = cand.score,
            company = %cand.company, "FIRED: {}", cand.title
        );

        *chosen_companies.entry(cand.company.clone()).or_insert(0) += 1;
        remaining -= 1;
        fired_any = true;
    }

    if fired_any {
        state.notify_ui();
    }
    Ok(())
}

/// Linearly relax the required score from `adaptive_start` at :00 to
/// `adaptive_end` at :59. Disabled => a flat `strong_min` bar.
fn adaptive_bar(rel: &crate::settings::Settings) -> f64 {
    if !rel.adaptive_threshold {
        return rel.strong_min;
    }
    let minute = ((now() % 3600) as f64) / 60.0; // 0..60
    let frac = (minute / 60.0).clamp(0.0, 1.0);
    rel.adaptive_start - (rel.adaptive_start - rel.adaptive_end) * frac
}

/// Reconstruct a minimal RawPost from a stored candidate for late drafting.
fn to_raw(c: &crate::model::Candidate) -> crate::model::RawPost {
    crate::model::RawPost {
        source: c.source.clone(),
        external_id: c.urn.clone(),
        url: c.url.clone(),
        title: c.title.clone(),
        company: c.company.clone(),
        location: c.location.clone(),
        body: c.body.clone(),
        posted_at: c.posted_at,
        apply: c.apply(),
        // Only used to regenerate a draft; the scorer never sees this one.
        synthetic_title: false,
    }
}

/// Hourly digest of marginal-tier matches. Small, separate from the instant path.
pub async fn digest(state: &AppState) -> anyhow::Result<()> {
    let mut batch = db::digest_batch(&state.pool, 50).await?;
    if batch.is_empty() {
        return Ok(());
    }
    batch.sort_by(|a, b| {
        b.live_priority()
            .partial_cmp(&a.live_priority())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    batch.truncate(10);
    notify::email_digest(&state.cfg, &batch).await?;
    let ids: Vec<i64> = batch.iter().map(|c| c.id).collect();
    db::mark_digested(&state.pool, &ids).await?;
    Ok(())
}
