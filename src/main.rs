mod api;
mod ats;
mod classify;
mod config;
mod db;
mod draft;
mod enrich;
mod errlog;
mod geo;
mod model;
mod notify;
mod pipeline;
mod release;
mod resume;
mod score;
mod sources;
mod settings;
mod state;
mod status;
mod tags;
mod text;
mod timeparse;
mod web;

use crate::config::Config;
use crate::sources::greenhouse::Greenhouse;
use crate::sources::lever::Lever;
use crate::sources::linkedin::{LinkedInGuest, Voyager};
use crate::sources::workday::Workday;
use crate::sources::JobSource;
use crate::settings::Settings;
use crate::state::AppState;
use rand::Rng;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Two sinks: stdout as before, and an in-memory ring buffer of warnings and
    // errors that the dashboard can show and copy. `docker compose logs` has
    // all of this already and is the wrong place to send someone — the six
    // lines that matter are scattered through thousands that don't.
    {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;
        tracing_subscriber::registry()
            .with(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "hiring_radar=info,tower_http=warn".into()),
            )
            .with(tracing_subscriber::fmt::layer())
            .with(errlog::CaptureLayer)
            .init();
    }

    let cfg_path = std::env::var("RADAR_CONFIG").unwrap_or_else(|_| "config.toml".into());
    let cfg = Arc::new(Config::load(&cfg_path)?);
    tracing::info!("loaded config for {}", cfg.profile.name);

    // DB path is env-driven so the container can keep state on a mounted volume.
    let db_path = std::env::var("RADAR_DB").unwrap_or_else(|_| "hiring.db".into());
    let pool = db::connect(&format!("sqlite:{db_path}?mode=rwc")).await?;
    tracing::info!(db = %db_path, "database ready");

    let http = reqwest::Client::builder()
        .user_agent(cfg.crawl.user_agent.clone())
        // 30s was not enough. Stripe and GitLab publish hundreds of jobs with
        // full descriptions in one response, and the body stopped arriving
        // mid-parse — which surfaced as "unreadable response" and read like a
        // schema change. Overridable because what is generous on a good
        // connection is still short on a bad one.
        .timeout(Duration::from_secs(
            std::env::var("RADAR_HTTP_TIMEOUT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(90),
        ))
        .build()?;

    let drafter: Arc<dyn draft::Drafter> = Arc::from(draft::build_drafter(&cfg.draft, http.clone()));
    let enricher: Arc<dyn enrich::Enricher> = Arc::from(enrich::build_enricher(&cfg.draft, http.clone()));
    let (events, _) = tokio::sync::broadcast::channel::<()>(64);

    // config.toml seeds the settings the first time only; after that the stored
    // row wins, so dashboard edits survive a restart.
    let live = match db::load_settings(&pool).await? {
        Some(s) => {
            tracing::info!("settings loaded from database");
            s
        }
        None => {
            let mut s = Settings::from_config(&cfg);
            s.sanitize();
            db::save_settings(&pool, &s).await?;
            tracing::info!("settings seeded from config.toml");
            s
        }
    };

    // Rehydrate the IDF corpus and vectorise the stored resume before any
    // crawl runs, so the first post scored after a restart is scored against
    // the same corpus as the last post before it.
    let mut matcher = resume::Matcher {
        corpus: db::load_corpus(&pool).await.unwrap_or_default(),
        resume: None,
    };
    matcher.rebuild_resume(&live.resume);
    tracing::info!(
        corpus_docs = matcher.corpus.docs,
        corpus_terms = matcher.corpus.df.len(),
        resume = matcher.resume.is_some(),
        "matcher ready"
    );

    // The profile is derived from the resume; keep a copy of the settings
    // before they move into the lock.
    let live_for_profile = live.clone();
    let state = AppState {
        pool: pool.clone(),
        cfg: cfg.clone(),
        http: http.clone(),
        drafter,
        enricher,
        events,
        settings: Arc::new(tokio::sync::RwLock::new(Arc::new(live))),
        matcher: Arc::new(tokio::sync::RwLock::new(Arc::new(matcher))),
        status: Arc::new(tokio::sync::RwLock::new(status::Status::new())),
        profile: Arc::new(tokio::sync::RwLock::new(Arc::new(
            ats::Profile::from_resume(&live_for_profile.resume, live_for_profile.years_experience),
        ))),
    };
    // Seed the status with which sources are on, so the panel is accurate
    // before the first crawl rather than showing three unknown rows.
    {
        let live = state.settings().await;
        state.status.write().await.apply_settings(&live);
    }

    // Bring the board onto the current scorer.
    //
    // Scores are stored, not computed on read, which is right — the number on
    // the card has to be the number the release engine acted on. The cost is
    // that changing how scoring works leaves a board full of numbers from the
    // old one, indistinguishable from the new and not comparable to them. A
    // deploy is exactly when that happens, so a deploy is when to fix it.
    // Rows you have already acted on are left alone; they are history.
    {
        let st = state.clone();
        tokio::spawn(async move {
            match pipeline::rescore_all(&st).await {
                Ok(n) if n > 0 => {
                    tracing::info!(changed = n, "board re-scored on start-up");
                    st.notify_ui();
                }
                Err(e) => tracing::warn!(%e, "start-up re-score failed"),
                _ => {}
            }
        });
    }

    // Rows that predate the region column, or whose location was worked out
    // after they were stored. Cheap, idempotent, and it means the region filter
    // is populated the first time you open it rather than only for new posts.
    match db::backfill_regions(&pool).await {
        Ok(n) if n > 0 => tracing::info!(filled = n, "regions backfilled"),
        Err(e) => tracing::warn!(%e, "region backfill failed"),
        _ => {}
    }

    // ---- assemble sources ----
    // Every source gets a loop unconditionally; each one checks the live
    // settings on every pass and no-ops when disabled. Enabling a source from
    // the dashboard therefore takes effect on its next tick, with no restart.
    let sources: Vec<(Box<dyn JobSource>, u64)> = vec![
        (Box::new(Greenhouse::new(http.clone())), cfg.crawl.broad_search_secs),
        (Box::new(Lever::new(http.clone())), cfg.crawl.broad_search_secs),
        (Box::new(LinkedInGuest::new(http.clone())), cfg.crawl.target_search_secs),
        (
            Box::new(Voyager::new(
                http.clone(),
                cfg.linkedin_voyager.clone(),
                cfg.crawl.user_agent.clone(),
            )),
            cfg.crawl.target_search_secs,
        ),
        // Workday listings are formal requisitions that sit open for weeks, so
        // this rides the slow tick. Polling it as often as the feed would mean
        // hundreds of requests an hour to learn nothing new.
        (Box::new(Workday::new(http.clone())), cfg.crawl.broad_search_secs),
    ];

    // ---- spawn a crawl loop per source ----
    for (source, interval) in sources {
        let st = state.clone();
        let jitter = cfg.crawl.jitter_secs;
        tokio::spawn(async move {
            crawl_loop(st, source, interval, jitter).await;
        });
    }

    // ---- release engine: re-evaluate the pool on a short tick ----
    {
        let st = state.clone();
        let tick = cfg.release.tick_secs.max(5);
        tokio::spawn(async move {
            let mut iv = tokio::time::interval(Duration::from_secs(tick));
            loop {
                // interval fires immediately on the first tick, so a restart
                // with a full pool doesn't idle for tick_secs before releasing.
                iv.tick().await;
                if let Err(e) = release::run(&st).await {
                    tracing::warn!(%e, "release tick failed");
                }
            }
        });
    }

    // ---- LLM enrichment, in the background ----
    //
    // Deliberately not inline with the crawl. Inference takes seconds per
    // posting, and a crawl that waits for it would turn a 90-second cycle into
    // an hour and miss the posts it exists to catch. The heuristics already ran
    // at ingest, so the board is filterable the whole time this is working
    // through the backlog — this only sharpens it.
    if state.enricher.is_llm() {
        let st = state.clone();
        tokio::spawn(async move {
            enrich_loop(st).await;
        });
    } else {
        tracing::info!(
            "no model configured for enrichment; filters use the heuristic reading \
             (set draft.provider = \"ollama\" in config.toml to sharpen them)"
        );
    }

    // ---- persist the IDF corpus periodically ----
    {
        let st = state.clone();
        tokio::spawn(async move {
            let mut iv = tokio::time::interval(Duration::from_secs(900));
            iv.tick().await; // the immediate first tick would write an empty corpus
            loop {
                iv.tick().await;
                let m = st.matcher().await;
                match db::save_corpus(&st.pool, &m.corpus, 3).await {
                    Ok(n) => tracing::debug!(terms = n, docs = m.corpus.docs, "corpus persisted"),
                    Err(e) => tracing::warn!(%e, "corpus persist failed"),
                }
            }
        });
    }

    // ---- housekeeping: drop rows nobody can see any more ----
    {
        let st = state.clone();
        let keep = cfg.server.prune_after_days.max(1) * 86400;
        tokio::spawn(async move {
            let mut iv = tokio::time::interval(Duration::from_secs(6 * 3600));
            loop {
                iv.tick().await;
                match db::prune_candidates(&st.pool, keep).await {
                    Ok(n) if n > 0 => tracing::info!(pruned = n, "old candidates dropped"),
                    Err(e) => tracing::warn!(%e, "prune failed"),
                    _ => {}
                }
            }
        });
    }

    // ---- hourly digest of marginal matches ----
    {
        let st = state.clone();
        tokio::spawn(async move {
            loop {
                sleep_to_next_hour().await;
                if let Err(e) = release::digest(&st).await {
                    tracing::warn!(%e, "digest failed");
                }
            }
        });
    }

    // ---- dashboard ----
    let listener = TcpListener::bind(&cfg.server.bind).await?;
    tracing::info!("dashboard on {}", cfg.server.base_url);
    axum::serve(listener, web::router(state)).await?;
    Ok(())
}

/// Poll one source forever: fetch -> dedup -> ingest new -> kick the release engine.
async fn crawl_loop(state: AppState, source: Box<dyn JobSource>, interval: u64, jitter: u64) {
    let name = source.name().to_string();
    loop {
        let live = state.settings().await;
        let enabled = source.is_enabled(&live);
        state
            .with_status(&name, |s| {
                s.enabled = enabled;
                s.begin_cycle();
            })
            .await;

        match source.fetch(&live).await {
            Ok(fetched) if fetched.posts.is_empty() => {
                let notes = fetched.notes.clone();
                tracing::debug!(source = %name, "nothing returned (source disabled or empty)");
                state.with_status(&name, |s| s.notes = notes).await;
            }
            Ok(fetched) => {
                let notes = fetched.notes.clone();
                let posts = fetched.posts;
                state
                    .with_status(&name, |s| {
                        s.notes = notes;
                        s.fetched = posts.len();
                        s.total_fetched += posts.len() as u64;
                    })
                    .await;
                // First non-empty cycle for this source is a backfill: record, don't notify.
                let bootstrapping = db::seen_count(&state.pool, &name).await.unwrap_or(0) == 0;
                let mut new_count = 0;
                let mut backfilled = 0;
                for post in posts {
                    let urn = post.urn();
                    let is_new = db::record_seen(&state.pool, &urn, &name, bootstrapping)
                        .await
                        .unwrap_or(false);
                    if !is_new {
                        continue; // already handled on an earlier cycle
                    }
                    // On the first crawl we still ingest — the radar needs history
                    // to show — but as 'backfilled', which can never fire.
                    match pipeline::ingest(&state, post, bootstrapping).await {
                        Err(e) => tracing::warn!(source = %name, %e, "ingest failed"),
                        Ok(outcome) => {
                            state.with_status(&name, |s| s.note_outcome(&outcome)).await;
                            if bootstrapping {
                                backfilled += 1;
                            } else {
                                new_count += 1;
                            }
                        }
                    }
                }
                state
                    .with_status(&name, |s| {
                        s.new_posts = if bootstrapping { backfilled } else { new_count };
                        if bootstrapping {
                            s.bootstrapped = true;
                        }
                    })
                    .await;
                if bootstrapping {
                    tracing::info!(source = %name, backfilled,
                        "bootstrapped (history on the radar, nothing notified)");
                    state.notify_ui();
                } else if new_count > 0 {
                    tracing::info!(source = %name, new_count, "new posts ingested");
                    // Instant path: don't wait for the next tick to fire exceptional matches.
                    if let Err(e) = release::run(&state).await {
                        tracing::warn!(%e, "post-crawl release failed");
                    }
                }
            }
            Err(e) => {
                tracing::warn!(source = %name, %e, "fetch failed");
                let msg = e.to_string();
                state.with_status(&name, |s| s.last_error = Some(msg)).await;
            }
        }

        let extra = if jitter > 0 {
            rand::thread_rng().gen_range(0..=jitter)
        } else {
            0
        };
        let wait = interval + extra;
        state.with_status(&name, |s| s.end_cycle(wait)).await;
        state.notify_ui();
        tokio::time::sleep(Duration::from_secs(wait)).await;
    }
}

/// Read un-enriched candidates with the model, a few at a time, forever.
///
/// Small batches with a pause between them, rather than draining the queue as
/// fast as the model will answer: this shares a machine with whatever else you
/// are running, and a local model given unbounded work will take every core it
/// can reach. The backlog is not urgent — the row is already on the board with
/// its heuristic reading.
async fn enrich_loop(state: AppState) {
    /// Postings per pass.
    const BATCH: i64 = 5;
    /// Between passes when there was work. Long enough to stay a background
    /// task rather than a load test.
    const BUSY_PAUSE_SECS: u64 = 10;
    /// Between passes when the queue was empty.
    const IDLE_PAUSE_SECS: u64 = 120;

    loop {
        let batch = db::unenriched(&state.pool, BATCH).await.unwrap_or_default();
        if batch.is_empty() {
            tokio::time::sleep(Duration::from_secs(IDLE_PAUSE_SECS)).await;
            continue;
        }

        for c in &batch {
            let post = model::RawPost {
                source: c.source.clone(),
                external_id: c.urn.clone(),
                url: c.url.clone(),
                title: c.title.clone(),
                company: c.company.clone(),
                location: c.location.clone(),
                body: c.body.clone(),
                posted_at: c.posted_at,
                apply: c.apply(),
                synthetic_title: c.source == "linkedin_voyager",
            };

            // Start from what is already stored and let the model refine it, so
            // a model with no opinion leaves the row exactly as it was.
            let mut facts = c.facts();
            match state.enricher.read(&post).await {
                Some(read) => facts.merge_from(read),
                None => tracing::debug!(id = c.id, "enricher had no opinion"),
            }

            let tags = tags::encode(&facts.stack);
            if let Err(e) = db::set_facts(&state.pool, c.id, &facts, tags).await {
                tracing::warn!(id = c.id, %e, "enrichment writeback failed");
                // Leave enriched_at NULL so it is retried rather than silently
                // dropped — but sleep first, because a failing database will
                // otherwise spin this loop.
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        }

        tracing::info!(read = batch.len(), "enriched a batch");
        state.notify_ui();
        tokio::time::sleep(Duration::from_secs(BUSY_PAUSE_SECS)).await;
    }
}

/// Sleep until the top of the next hour (for the digest job).
async fn sleep_to_next_hour() {
    let now = model::now();
    let secs_into_hour = now % 3600;
    let wait = (3600 - secs_into_hour).max(1) as u64;
    tokio::time::sleep(Duration::from_secs(wait)).await;
}
