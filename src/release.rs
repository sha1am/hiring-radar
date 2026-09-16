use crate::{db, notify};
use crate::state::AppState;
use std::collections::HashMap;

/// How many times a candidate may be claimed and not delivered before it is
/// parked as undeliverable.
///
/// Three, then it stops. An unreachable ntfy server and a wrong SMTP password
/// are not transient faults, and a pool that retries them every thirty seconds
/// until everything expires produces a log of one repeated failure and no
/// signal. Parking it puts the word "undeliverable" on the dashboard, where it
/// is a question you can answer.
const MAX_SEND_ATTEMPTS: i64 = 3;

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
        // Exceptional passes the bar by definition; so does anything naming a
        // technology from the instant list, which is what putting one there
        // means — you asked for these by name, not by score.
        let is_exceptional = cand.tier == "exceptional" || cand.instant;

        // Adaptive gate (exceptional always passes). Recomputed each time round
        // because `remaining` is what it reads: a pass that sends three in a row
        // raises its own bar as it goes.
        if !is_exceptional && cand.score < adaptive_bar(rel, remaining) {
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
            let post = cand.as_post();
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

        // Dispatch: push is the ping, email is the record.
        let mut delivered: Vec<&str> = Vec::new();
        match notify::push_ntfy(&state.http, &state.cfg, &cand).await {
            Ok(()) => delivered.push("ntfy"),
            Err(e) => tracing::warn!(id = cand.id, %e, "ntfy failed"),
        }
        match notify::email_self(&state.cfg, &cand).await {
            Ok(()) => delivered.push("email"),
            Err(e) => tracing::warn!(id = cand.id, %e, "self-email failed"),
        }

        // Nothing delivered is not a notification. Hand the slot back, or the
        // budget goes on posts nobody was told about — and because the claim is
        // unique per candidate, it could never be re-sent afterwards.
        if delivered.is_empty() {
            let failures = db::unfire(&state.pool, cand.id).await?;
            if failures >= MAX_SEND_ATTEMPTS {
                db::set_status(&state.pool, cand.id, "undeliverable").await?;
                tracing::error!(
                    id = cand.id, failures, company = %cand.company,
                    "giving up after {failures} failed sends — check ntfy and SMTP: {}",
                    cand.title
                );
            } else {
                tracing::error!(
                    id = cand.id, failures,
                    "nothing delivered; slot returned, will retry: {}", cand.title
                );
            }
            continue;
        }

        db::record_channel(&state.pool, cand.id, &delivered.join("+")).await?;

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

/// How good a post has to be to spend one of the remaining slots.
///
/// This used to interpolate on minutes past the top of the UTC hour, which
/// measured the wrong thing entirely: the budget is a *rolling* sixty minutes,
/// so there is no top of the hour to be early in. At :59 with four slots already
/// spent it relaxed the bar to its most generous — inviting exactly the posts it
/// had no room for — and at :01 with a completely untouched budget it was at its
/// strictest, refusing to spend a slot it was about to lose anyway.
///
/// What actually decides whether a post is worth a slot is how many slots there
/// are. The last one is expensive: spend it on something mediocre and the good
/// post twenty minutes later waits an hour. The fourth of four costs almost
/// nothing, and refusing to spend it is how a quiet night passes with an empty
/// inbox and a pool full of decent matches.
///
/// So: `adaptive_start` (strict) when the budget is nearly gone, `adaptive_end`
/// (relaxed) when it is untouched, linear in between. Disabled => flat
/// `strong_min`.
fn adaptive_bar(rel: &crate::settings::Settings, remaining: i64) -> f64 {
    if !rel.adaptive_threshold {
        return rel.strong_min;
    }
    let cap = rel.per_hour_cap.max(1) as f64;
    let free = (remaining.max(0) as f64 / cap).clamp(0.0, 1.0);
    rel.adaptive_start - (rel.adaptive_start - rel.adaptive_end) * free
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    fn rel() -> Settings {
        let mut s: Settings = serde_json::from_str("{}").unwrap();
        s.adaptive_threshold = true;
        s.adaptive_start = 90.0;
        s.adaptive_end = 70.0;
        s.per_hour_cap = 4;
        s
    }

    #[test]
    fn the_last_slot_is_the_expensive_one() {
        let s = rel();
        // One left of four: nearly strict. All four free: as generous as it gets.
        assert!(adaptive_bar(&s, 1) > adaptive_bar(&s, 4));
        assert_eq!(adaptive_bar(&s, 4), s.adaptive_end);
        assert_eq!(adaptive_bar(&s, 0), s.adaptive_start);
    }

    #[test]
    fn the_bar_does_not_move_with_the_wall_clock() {
        // The budget is a rolling hour. The old version read the UTC minute,
        // which meant the bar swung between its extremes on a schedule that had
        // nothing to do with how much budget was left.
        let s = rel();
        let a = adaptive_bar(&s, 2);
        let b = adaptive_bar(&s, 2);
        assert_eq!(a, b);
        assert!((a - 80.0).abs() < 1e-9, "half the budget should be half way: {a}");
    }

    #[test]
    fn switching_it_off_gives_a_flat_bar() {
        let mut s = rel();
        s.adaptive_threshold = false;
        assert_eq!(adaptive_bar(&s, 0), s.strong_min);
        assert_eq!(adaptive_bar(&s, 4), s.strong_min);
    }

    #[test]
    fn a_cap_of_zero_does_not_divide_by_zero() {
        let mut s = rel();
        s.per_hour_cap = 0;
        assert!(adaptive_bar(&s, 0).is_finite());
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
