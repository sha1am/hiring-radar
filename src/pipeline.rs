use crate::model::{now, RawPost, Tier};
use crate::resume::tokenize;
use crate::status::Outcome;
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
pub async fn ingest(
    state: &AppState,
    post: RawPost,
    backfill: bool,
) -> anyhow::Result<Outcome> {
    // 1. Is this actually a hiring post?
    if !classify::is_hiring(&post) {
        return Ok(Outcome::NotHiring);
    }

    // Snapshot the settings once: a save mid-ingest must not score a post
    // against one profile and tier it against another.
    let live = state.settings().await;

    // Sources that report no location field (feed posts, most notably) get one
    // inferred from their text. This has to happen BEFORE scoring, because the
    // location filter and the dashboard both read post.location — otherwise
    // "only India" silently lets every feed post through for want of a value.
    let mut post = post;
    if post
        .location
        .as_deref()
        .map(|l| l.trim().is_empty())
        .unwrap_or(true)
    {
        post.location = crate::geo::describe(&format!("{} {}", post.title, post.body));
    }
    let post = post;

    // 2. Location gate. Runs before scoring so a post in the wrong place is
    //    reported as exactly that, instead of surfacing as an unexplained zero.
    if crate::score::location_verdict(&post, &live) == crate::score::LocationVerdict::Rejected {
        tracing::debug!(
            location = post.location.as_deref().unwrap_or("unknown"),
            "dropped: outside configured locations"
        );
        return Ok(Outcome::WrongLocation);
    }

    // 2b. Stack gate, for the same reason and in the same place as location:
    //     a post dropped for being in the wrong language should say so, not
    //     surface as an unexplained low score.
    if crate::score::stack_verdict(&post, &live) == crate::score::StackVerdict::Rejected {
        tracing::debug!(title = %post.title, "dropped: not in the configured stack");
        return Ok(Outcome::WrongStack);
    }

    // 3. Score against the profile + resume.
    let matcher = state.matcher().await;
    let scorer = LexicalScorer;
    let (score, matched) = scorer.score(&post, &live, &matcher);

    // Every classified post feeds the IDF corpus, including ones about to be
    // dropped — a post we don't want still tells us which terms are common.
    state.observe_corpus(&tokenize(&post.haystack())).await;

    // 4. Tier — below the floor is dropped entirely (never stored).
    let Some(tier) = Tier::from_score(score, &live) else {
        return Ok(Outcome::BelowFloor { score });
    };

    let detected = now();
    let prio = priority(score, post.posted_at, detected, detected);
    let settle_until = detected + tier.settle_secs(&live);
    let expires_at = detected + live.candidate_ttl_secs;

    // 5. Draft-ahead for anything that could fire, so the draft is ready on arrival.
    //    Backfill can't fire, so drafting it would be wasted work (and, with the
    //    ollama drafter, a very slow first crawl).
    let (draft_subject, draft_body) = if !backfill && matches!(tier, Tier::Exceptional | Tier::Strong) {
        let (s, b) = state
            .drafter
            .draft(&post, &live, &state.cfg.profile.name, &state.cfg.profile.email)
            .await;
        (Some(s), Some(b))
    } else {
        (None, None)
    };

    // Heuristic reading, now, so the filters work on a fresh install with no
    // model configured. The LLM pass refines this in the background.
    let facts = crate::enrich::heuristic(&post);

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
        match_terms: if matched.is_empty() { None } else { Some(matched.join(", ")) },
        tags: crate::tags::encode(&facts.stack),
        role: facts.role.clone(),
        level: facts.level.clone(),
        years_min: facts.years_min,
        years_max: facts.years_max,
        work_mode: facts.work_mode.clone(),
        employment: facts.employment.clone(),
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
    Ok(Outcome::Stored { score })
}
