# Hiring Radar

Self-hosted radar for new hiring posts. It continuously watches your target
sources, decides on each post *is this a hiring post → how well does it match me →
should it interrupt me right now*, and the instant a good one lands it pushes your
phone (ntfy), emails you the record with a ready-to-edit draft, and drops a card on
a live dashboard. Rust, SQLite, $0 to run.

## The one interesting problem

You want alerts **the moment** a post is detected — but you also don't want 40
pings an hour. Instant-fire and a rate cap fight each other: if you fire on the
first decent post at 2:05 you can burn your last slot right before a great post at
2:25, and you can't un-send a notification. This is an online decision under a
budget you can't take back.

The resolution is a **tiered release policy** over a **rolling-60-minute budget**,
not one rule (`src/release.rs`):

| Tier | Score | Behaviour | Latency |
|------|-------|-----------|---------|
| Exceptional | ≥ 92 | fire immediately, jump the queue | seconds |
| Strong | 75–92 | short **settling window**, then release best-of-window if a slot is free | a few min |
| Marginal | 55–75 | never instant — hourly digest only | ≤ 1h |
| —     | < 55  | dropped, never stored | — |

Plus: a hard cap of **4 sent/hour**, a **per-poster cap** (one recruiter can't eat
all four slots), **rollover** (a strong post that loses this hour keeps competing
until it expires, ~12h), priority = **match × freshness** (2h half-life, so a newer
post of equal match wins the slot), and an optional **adaptive threshold** that
relaxes the bar as the hour drains so a quiet hour doesn't waste the budget.

## How the pieces map to files

```
src/
  main.rs            wiring: config, DB, one crawl loop per source, release ticker, digest, server
  config.rs          config.toml + ENV secrets
  model.rs           RawPost, ApplyChannel, Tier, Candidate; unix-time helpers
  db.rs              SQLite: schema, dedup/seen, candidate insert, budget + per-poster queries
  pipeline.rs        ingest: classify → score → tier → draft-ahead → persist
  classify.rs        is-this-a-hiring-post filter (drops "open to work" posts)
  score.rs           LexicalScorer (swappable) + freshness-weighted priority()
  draft.rs           TemplateDrafter (offline) + OllamaDrafter (local LLM); channel-shaped
  release.rs         >>> the tiered release engine <<<
  notify.rs          ntfy push, self-alert email, outbound application send, digest
  web.rs             axum dashboard: SSE live updates, editable drafts, send/dismiss/applied
  sources/
    greenhouse.rs        public boards API — safe, real, no ban risk (start here)
    linkedin_guest.rs    guest jobs endpoint — no login, lower risk, HTML parsed
    linkedin_voyager.rs  authenticated content search — real hiring posts, FRAGILE
```

Flow: `crawl → dedup(URN) → classify → score → tier(drop if <floor) → draft-ahead
(strong+) → pool`. Then, on every tick and right after each crawl: `expire →
check budget → walk pool best-first under per-poster cap + adaptive bar → fire →
ntfy + email + dashboard SSE`. You then edit → Send / Copy+Open / Applied /
Dismiss, which feeds the feedback table.

## Setup

Prereqs: a recent Rust toolchain (`rustup`), and for email a Gmail **app
password** (not your login password).

```bash
cp config.example.toml config.toml
# edit config.toml: your profile, greenhouse boards, linkedin queries, ntfy topic, smtp user
```

Pick an **unguessable** ntfy topic (anyone who knows it can read your alerts), then
subscribe to it in the ntfy app on your phone.

Secrets go in the environment, never in the file:

```bash
export SMTP_PASSWORD="your-gmail-app-password"
# only if you enable the voyager source:
export LI_AT="value-of-your-li_at-cookie"
export LI_JSESSIONID="value-of-your-JSESSIONID-cookie"
```

Run:

```bash
cargo run --release
# dashboard: http://127.0.0.1:8080
```

On first run every source **bootstraps** — existing posts are marked seen without
notifying, so you don't get blasted with history. Only genuinely-new posts after
launch fire.

## Tuning

Everything lives in `[release]` in `config.toml`: the caps, the tier thresholds,
the settling window, the TTL, and the adaptive bar. Start with Greenhouse only to
watch the whole pipeline work, then add LinkedIn.

## Upgrading the scorer / drafter

Both are traits. To replace keyword scoring with embeddings, implement `Scorer`
with fastembed cosine similarity (or an LLM call) and swap it in `pipeline.rs`. To
use a local LLM for drafts, set `[draft] provider = "ollama"` and run Ollama.

## Honest caveats

- **LinkedIn Voyager is the fragile part.** The `query_id` and JSON shape change
  when LinkedIn ships. It's disabled by default; when you enable it, expect to
  grab a fresh `query_id` from DevTools and adjust `looks_like_post` in
  `linkedin_voyager.rs` to the live field names. Authenticated scraping also
  carries real account-ban risk — use one dedicated account, keep polling gentle,
  and keep the jitter on.
- **The guest endpoint returns formal listings**, not personal "we're hiring"
  posts. It's the safe, always-on LinkedIn source; Voyager is what gets the
  people-posts.
- **Auto-send is email-only.** LinkedIn DMs are always drafted for you to send by
  hand (highest ban risk); the dashboard gives you Copy draft + Open profile.
- The lexical scorer is deliberately simple. It's good enough to route; embeddings
  are a clean upgrade when you want fewer false positives.
```
