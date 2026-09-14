mod classify;
mod config;
mod db;
mod draft;
mod model;
mod notify;
mod pipeline;
mod release;
mod score;
mod sources;
mod state;
mod timeparse;
mod web;

use crate::config::Config;
use crate::sources::greenhouse::Greenhouse;
use crate::sources::linkedin_guest::LinkedInGuest;
use crate::sources::linkedin_voyager::Voyager;
use crate::sources::JobSource;
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

    let state = AppState {
        pool: pool.clone(),
        cfg: cfg.clone(),
        http: http.clone(),
        drafter,
        events,
    };

    // ---- assemble sources ----
    let mut sources: Vec<(Box<dyn JobSource>, u64)> = Vec::new();

    if cfg.greenhouse.enabled && !cfg.greenhouse.boards.is_empty() {
        sources.push((
            Box::new(Greenhouse::new(http.clone(), cfg.greenhouse.boards.clone())),
            cfg.crawl.broad_search_secs,
        ));
    }
    if !cfg.crawl.linkedin_queries.is_empty() {
        sources.push((
            Box::new(LinkedInGuest::new(http.clone(), cfg.crawl.linkedin_queries.clone())),
            cfg.crawl.target_search_secs,
        ));
    }
    if cfg.linkedin_voyager.enabled {
        sources.push((
            Box::new(Voyager::new(
                http.clone(),
                cfg.linkedin_voyager.clone(),
                cfg.crawl.user_agent.clone(),
                cfg.profile.titles.clone(),
            )),
            cfg.crawl.target_search_secs,
        ));
    }

    if sources.is_empty() {
        tracing::warn!("no sources enabled — check [greenhouse] / [[crawl.linkedin_queries]] in config");
    }

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
                iv.tick().await;
                if let Err(e) = release::run(&st).await {
                    tracing::warn!(%e, "release tick failed");
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
        match source.fetch().await {
            Ok(posts) => {
                // First non-empty cycle for this source is a backfill: record, don't notify.
                let bootstrapping = db::seen_count(&state.pool, &name).await.unwrap_or(0) == 0;
                let mut new_count = 0;
                for post in posts {
                    let urn = post.urn();
                    let is_new = db::record_seen(&state.pool, &urn, &name, bootstrapping)
                        .await
                        .unwrap_or(false);
                    if bootstrapping || !is_new {
                        continue; // historical on first run, or already handled
                    }
                    if let Err(e) = pipeline::ingest(&state, post).await {
                        tracing::warn!(source = %name, %e, "ingest failed");
                    } else {
                        new_count += 1;
                    }
                }
                if bootstrapping {
                    tracing::info!(source = %name, "bootstrapped (existing posts marked seen)");
                } else if new_count > 0 {
                    tracing::info!(source = %name, new_count, "new posts ingested");
                    // Instant path: don't wait for the next tick to fire exceptional matches.
                    if let Err(e) = release::run(&state).await {
                        tracing::warn!(%e, "post-crawl release failed");
                    }
                }
            }
            Err(e) => tracing::warn!(source = %name, %e, "fetch failed"),
        }

        let extra = if jitter > 0 {
            rand::thread_rng().gen_range(0..=jitter)
        } else {
            0
        };
        tokio::time::sleep(Duration::from_secs(interval + extra)).await;
    }
}

/// Sleep until the top of the next hour (for the digest job).
async fn sleep_to_next_hour() {
    let now = model::now();
    let secs_into_hour = now % 3600;
    let wait = (3600 - secs_into_hour).max(1) as u64;
    tokio::time::sleep(Duration::from_secs(wait)).await;
}
