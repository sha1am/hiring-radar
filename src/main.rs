mod classify;
mod config;
mod db;
mod draft;
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
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "hiring_radar=info,tower_http=warn".into()),
        )
        .init();

    let cfg_path = std::env::var("RADAR_CONFIG").unwrap_or_else(|_| "config.toml".into());
    let cfg = Arc::new(Config::load(&cfg_path)?);
    tracing::info!("loaded config for {}", cfg.profile.name);

    // DB path is env-driven so the container can keep state on a mounted volume.
    let db_path = std::env::var("RADAR_DB").unwrap_or_else(|_| "hiring.db".into());
    let pool = db::connect(&format!("sqlite:{db_path}?mode=rwc")).await?;
    tracing::info!(db = %db_path, "database ready");

    let http = reqwest::Client::builder()
        .user_agent(cfg.crawl.user_agent.clone())
        .timeout(Duration::from_secs(30))
        .build()?;

    let drafter: Arc<dyn draft::Drafter> = Arc::from(draft::build_drafter(&cfg.draft, http.clone()));
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

    let state = AppState {
        pool: pool.clone(),
        cfg: cfg.clone(),
        http: http.clone(),
        drafter,
        events,
        settings: Arc::new(tokio::sync::RwLock::new(Arc::new(live))),
        matcher: Arc::new(tokio::sync::RwLock::new(Arc::new(matcher))),
        status: Arc::new(tokio::sync::RwLock::new(status::Status::new())),
    };
    // Seed the status with which sources are on, so the panel is accurate
    // before the first crawl rather than showing three unknown rows.
    {
        let live = state.settings().await;
        state.status.write().await.apply_settings(&live);
    }

    // ---- assemble sources ----
    // Every source gets a loop unconditionally; each one checks the live
    // settings on every pass and no-ops when disabled. Enabling a source from
    // the dashboard therefore takes effect on its next tick, with no restart.
    let sources: Vec<(Box<dyn JobSource>, u64)> = vec![
        (Box::new(Greenhouse::new(http.clone())), cfg.crawl.broad_search_secs),
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

/// Sleep until the top of the next hour (for the digest job).
async fn sleep_to_next_hour() {
    let now = model::now();
    let secs_into_hour = now % 3600;
    let wait = (3600 - secs_into_hour).max(1) as u64;
    tokio::time::sleep(Duration::from_secs(wait)).await;
}
