use crate::model::{now, RawPost, Tier};
use crate::score::{priority, LexicalScorer, Scorer};
use crate::state::AppState;
use crate::{classify, db};

/// Turn one freshly-detected post into a scored candidate.
/// Called only for URNs we've never seen (dedup happens in the crawl loop).
///
/// `backfill` marks the first crawl of a source: the post is scored and stored
/// so the radar has history to show, but lands in 'backfilled' status, which
/// `db::eligible` excludes — so it can never fire a notification. Without this
/// the dashboard is empty until something new is posted.
pub async fn ingest(state: &AppState, post: RawPost, backfill: bool) -> anyhow::Result<()> {
    // 1. Is this actually a hiring post?
    if !classify::is_hiring(&post) {
        return Ok(());
    }

    // 2. Score against the profile.
    let scorer = LexicalScorer;
    let score = scorer.score(&post, &state.cfg.profile);

    // 3. Tier — below the floor is dropped entirely (never stored).
    let Some(tier) = Tier::from_score(score, &state.cfg.release) else {
        return Ok(());
    };

    let detected = now();
    let prio = priority(score, post.posted_at, detected, detected);
    let settle_until = detected + tier.settle_secs(&state.cfg.release);
    let expires_at = detected + state.cfg.release.candidate_ttl_secs;

    // 4. Draft-ahead for anything that could fire, so the draft is ready on arrival.
    //    Backfill can't fire, so drafting it would be wasted work (and, with the
    //    ollama drafter, a very slow first crawl).
    let (draft_subject, draft_body) = if !backfill && matches!(tier, Tier::Exceptional | Tier::Strong) {
        let (s, b) = state.drafter.draft(&post, &state.cfg.profile).await;
        (Some(s), Some(b))
    } else {
        (None, None)
    };

    let nc = db::NewCandidate {
        urn: post.urn(),
        source: post.source.clone(),
        url: post.url.clone(),
        title: post.title.clone(),
        company: post.company.clone(),
        location: post.location.clone(),
        body: post.body.clone(),
        score,
        priority: prio,
        tier: tier.as_str().to_string(),
        detected_at: detected,
        posted_at: post.posted_at,
        expires_at,
        settle_until,
        apply_kind: post.apply.kind().to_string(),
        apply_target: post.apply.target(),
        draft_subject,
        draft_body,
    };
    let status = if backfill { "backfilled" } else { "scored" };
    db::insert_candidate(&state.pool, &nc, status).await?;

    if backfill {
        tracing::debug!(tier = tier.as_str(), score, company = %post.company,
            "backfilled: {}", post.title);
    } else {
        tracing::info!(tier = tier.as_str(), score, company = %post.company,
            "scored: {}", post.title);
    }
    Ok(())
}
