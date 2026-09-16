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

    // 2c. Discipline gate. One board per company means the whole payroll comes
    //     down the same pipe — Optum publishes nurses, Delhivery publishes
    //     warehouse staff, everybody publishes recruiters — and none of it is
    //     evidence about you in either direction. Dropped here rather than
    //     scored low, because a job in another department is a category error,
    //     not a weak match, and because storing them buries every facet list on
    //     the dashboard under words from someone else's career.
    let profile = state.profile().await;
    if profile.is_engineering() {
        if let Some(field) = crate::enrich::off_discipline(&post.title) {
            tracing::debug!(title = %post.title, field, "dropped: another discipline");
            return Ok(Outcome::OffDiscipline);
        }
    }

    // Heuristic reading, first, because the ATS scores against it — and so the
    // filters work on a fresh install with no model configured. The LLM pass
    // refines both later.
    let facts = crate::enrich::heuristic(&post);

    // 3. Score it.
    //
    // The ATS when there is enough of a profile to assess against, the lexical
    // scorer otherwise. Not a preference — the ATS compares structured facts on
    // both sides, and with no resume loaded there is nothing on one of them. A
    // confident-looking number derived from nothing is worse than a rough one
    // that admits what it is.
    let matcher = state.matcher().await;

    let (score, matched, assessment) = if profile.is_usable() {
        let a = crate::ats::assess(&profile, &facts, &post, &live);
        // `met` stands in for the old term list: what the posting asked for and
        // you have, which is a far better answer to "why did this fire" than a
        // list of words the two documents share.
        (a.score, a.met.clone(), Some(a))
    } else {
        let (score, matched) = LexicalScorer.score(&post, &live, &matcher);
        (score, matched, None)
    };

    // Every classified post feeds the IDF corpus, including ones about to be
    // dropped — a post we don't want still tells us which terms are common.
    state.observe_corpus(&tokenize(&post.haystack())).await;

    // 4. Is this one of the things you asked to be interrupted for?
    //
    //    "Tell me about every Go job" is not the same request as "show me the
    //    best jobs", and the scoring machinery answers the second. A Go role at
    //    a company you have never heard of, pitched two levels above you, is
    //    still a Go role you wanted to see — so the instant list is checked
    //    against the technologies named in the posting and, when it hits, the
    //    posting skips the floor and the settling window entirely.
    let watched = live.instant_stack.iter().find(|w| {
        facts
            .stack
            .iter()
            .any(|t| t.eq_ignore_ascii_case(w.trim()))
    });

    // 5. Tier — below the floor is dropped entirely (never stored), unless it
    //    is something you are watching for, in which case the floor is the
    //    wrong instrument: it measures fit, and you asked by name.
    let tier = match (Tier::from_score(score, &live), watched) {
        (Some(t), _) => t,
        (None, Some(_)) => Tier::Marginal,
        (None, None) => return Ok(Outcome::BelowFloor { score }),
    };

    let detected = now();
    let prio = priority(score, post.posted_at, detected, detected);
    // Settling exists to release the best of a batch rather than the first of
    // it. For a watched technology there is nothing to choose between.
    let settle_until = if watched.is_some() {
        detected
    } else {
        detected + tier.settle_secs(&live)
    };
    let expires_at = detected + live.candidate_ttl_secs;

    // 5. Draft-ahead for anything that could fire, so the draft is ready on arrival.
    //    Backfill can't fire, so drafting it would be wasted work (and, with the
    //    ollama drafter, a very slow first crawl).
    let (draft_subject, draft_body) = if !backfill
        && (watched.is_some() || matches!(tier, Tier::Exceptional | Tier::Strong))
    {
        let (s, b) = state
            .drafter
            .draft(&post, &live, &state.cfg.profile.name, &state.cfg.profile.email)
            .await;
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
        match_terms: if matched.is_empty() { None } else { Some(matched.join(", ")) },
        tags: crate::tags::encode(&facts.stack),
        role: facts.role.clone(),
        level: facts.level.clone(),
        years_min: facts.years_min,
        years_max: facts.years_max,
        work_mode: facts.work_mode.clone(),
        employment: facts.employment.clone(),
        // Deterministic from the gazetteer, not from the model — a region is a
        // lookup, and asking an LLM to do lookups is how you get Bengaluru
        // filed under Europe on a bad day.
        verdict: assessment.as_ref().and_then(|a| a.verdict.clone()),
        reason: assessment.as_ref().map(|a| a.reason.clone()),
        missing: assessment.as_ref().and_then(|a| crate::tags::encode(&a.missing)),
        dimensions: assessment
            .as_ref()
            .and_then(|a| serde_json::to_string(&a.dimensions).ok()),
        region: post
            .location
            .as_deref()
            .and_then(crate::geo::region_of)
            .or_else(|| crate::geo::region_of(&post.haystack()))
            .map(str::to_string),
        apply_kind: post.apply.kind().to_string(),
        apply_target: post.apply.target(),
        draft_subject,
        draft_body,
        instant: watched.is_some(),
    };
    let status = if backfill { "backfilled" } else { "scored" };
    db::insert_candidate(&state.pool, &nc, status).await?;

    if backfill {
        tracing::debug!(tier = tier.as_str(), score, company = %post.company,
            "backfilled: {}", post.title);
    } else if let Some(w) = watched {
        tracing::info!(tier = tier.as_str(), score, company = %post.company, watching = %w,
            "scored (instant): {}", post.title);
    } else {
        tracing::info!(tier = tier.as_str(), score, company = %post.company,
            "scored: {}", post.title);
    }
    Ok(Outcome::Stored { score })
}

/// Re-assess everything on the board against the current profile.
///
/// Uploading a resume changes what every score means, and without this the
/// board keeps showing numbers computed against whatever it knew before — a
/// hundred rows scored by the lexical fallback sitting next to new ones scored
/// by the ATS, indistinguishable and not comparable. That is worse than either
/// alone, because the sort order silently mixes two scales.
///
/// Rows you have already acted on are left alone: re-scoring something you
/// dismissed cannot change your mind for you, and re-scoring something you have
/// applied to would rewrite history.
pub async fn rescore_all(state: &AppState) -> anyhow::Result<usize> {
    let live = state.settings().await;
    let profile = state.profile().await;
    if !profile.is_usable() {
        // Nothing to score against; the existing numbers are the best available.
        return Ok(0);
    }

    let rows = db::all_scorable(&state.pool).await?;
    let mut changed = 0usize;

    for c in &rows {
        let post = c.as_post();
        // The stored facts, not a fresh extraction: the LLM pass may have
        // sharpened them, and throwing that away to re-derive from the body
        // would undo work already paid for.
        let facts = c.facts();
        let a = crate::ats::assess(&profile, &facts, &post, &live);

        // A row that drops below the floor stays visible rather than
        // disappearing. It is history now, and silently deleting the board
        // because you edited your resume is alarming rather than helpful.
        let tier = crate::model::Tier::from_score(a.score, &live)
            .map(|t| t.as_str().to_string())
            .unwrap_or_else(|| "marginal".to_string());

        // The instant list is re-read here too: adding "rust" to it should
        // light up the Rust roles already on the board, not only the next ones
        // to arrive.
        let instant = live
            .instant_stack
            .iter()
            .any(|w| facts.stack.iter().any(|t| t.eq_ignore_ascii_case(w.trim())));

        if (a.score - c.score).abs() > 0.05 || c.verdict != a.verdict {
            changed += 1;
        }
        db::set_assessment(&state.pool, c.id, &a, &tier, instant).await?;
    }

    tracing::info!(rows = rows.len(), changed, "board re-scored against the profile");
    Ok(changed)
}
