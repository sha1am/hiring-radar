# Hiring Radar

Self-hosted radar for hiring posts. It watches your target sources, decides on
each post *is this a hiring post → how well does it match me → should it
interrupt me right now*, and the instant a good one lands it pushes your phone
(ntfy), emails you the record with a ready-to-edit draft, and drops a card on a
live dashboard. Everything you didn't get alerted about is still on the radar
view, so the board is useful on the first run rather than empty until something
new happens to be posted.

Rust, SQLite, one container, $0 to run.

## Running it

```bash
cp config.example.toml config.toml   # your profile, boards, ntfy topic, smtp user
cp .env.example .env                 # SMTP_PASSWORD, and LI_* only if you use voyager
docker compose up -d --build
```

Dashboard on <http://127.0.0.1:8080>. That's the whole operating procedure —
`docker compose up -d` and nothing else. First build compiles ~400 crates;
after that, source edits rebuild in seconds because dependencies live in their
own layer.

Pick an **unguessable** ntfy topic (anyone who knows it can read your alerts),
then subscribe to it in the ntfy app on your phone.

State — the queue, the seen-set, the rolling budget, your settings and the
resume — lives on a named volume at `/data`, so it survives
`docker compose down`.

> **The dashboard has no authentication.** The process inside the container
> binds `0.0.0.0` because it must, and compose publishes to `127.0.0.1` only.
> That loopback binding is the only thing standing between the internet and a
> button that sends email as you. Don't change the left side of the port
> mapping without putting an auth proxy in front.

## The one interesting problem

You want alerts **the moment** a post is detected — but you also don't want 40
pings an hour. Instant-fire and a rate cap fight each other: if you fire on the
first decent post at 2:05 you can burn your last slot right before a great post
at 2:25, and you can't un-send a notification. This is an online decision under
a budget you can't take back.

The resolution is a **tiered release policy** over a **rolling-60-minute
budget**, not one rule (`src/release.rs`):

| Tier | Score | Behaviour | Latency |
|------|-------|-----------|---------|
| Exceptional | ≥ 92 | fire immediately, jump the queue | seconds |
| Strong | 75–92 | short **settling window**, then release best-of-window if a slot is free | a few min |
| Marginal | 55–75 | never instant — hourly digest only | ≤ 1h |
| —     | < 55  | dropped, never stored | — |

Plus: a hard cap of **4 sent/hour**, a **per-poster cap** (one recruiter can't
eat all four slots), **rollover** (a strong post that loses this hour keeps
competing until it expires, ~12h), priority = **match × freshness** (2h
half-life, so a newer post of equal match wins the slot), and an optional
**adaptive threshold** that relaxes the bar as the hour drains so a quiet hour
doesn't waste the budget.

Every threshold in that table is editable in Settings without a rebuild.

## Matching against your resume

Upload a PDF (or paste the text) in **Settings → Your resume**, and posts are
scored by how much they look like *your* work rather than against a keyword
list you have to maintain.

It's IDF-weighted cosine similarity over unigrams and bigrams, averaged with
coverage of your resume's most distinctive terms. Bigrams matter more than they
look — "distributed systems" and "event driven" are what separate one backend
role from another, and unigrams alone dissolve them into common words. The IDF
comes from the posts this instance has actually seen, so distinctive phrases
earn weight and "responsibilities" doesn't, with nobody curating anything. Its
influence ramps in with corpus size, because on a handful of documents document
frequencies are noise.

Structural signals — title, location, seniority — stay separate and are never
expressed through the resume. A resume can't say "senior, in Bengaluru", and
scoring one with the other is how you end up alerting on a Berlin internship
that happens to mention Kafka.

A `resume weight` slider blends resume similarity against your keyword list, so
it's a dial rather than a switch. With no resume loaded, keyword scoring is
unchanged. Cards and radar rows show which of your resume's terms a post hit —
the difference between "94" and "94, because it wants exactly the Kafka and
Kubernetes work you've been doing".

It is deliberately not an LLM call: this runs on the instant path for every
post, and the score you see has to be the one the release engine acted on.
Nothing about your resume leaves the container.

## Settings

`/settings` edits, live, with no restart:

- **Matching** — titles, keywords, locations, remote-ok, seniority,
  dealbreakers, min salary, resume + resume weight.
- **Sources** — Greenhouse board tokens, LinkedIn queries, per-source on/off.
- **Alert budget** — hourly cap, per-company cap, tier thresholds, settling
  window, rollover TTL, adaptive bar, radar window.

`config.toml` is mounted read-only, so it is the *seed* and the only place
secrets are referenced; a single JSON row in SQLite is the live truth. On boot
the stored row wins if it exists. Values are clamped on save, so the tier ladder
can't be inverted and the cap can't be zeroed.

Voyager credentials (`LI_AT`, `LI_JSESSIONID`) and its `query_id` stay in
config/env — secrets and a fragile constant don't belong in a web form. The
dashboard toggle only decides whether to use them.

## How the pieces map to files

```
src/
  main.rs            wiring: config, DB, one crawl loop per source, release ticker, digest, server
  config.rs          config.toml + ENV secrets + deployment overrides
  settings.rs        live, user-editable configuration (persisted as JSON in SQLite)
  model.rs           RawPost, ApplyChannel, Tier, Candidate; unix-time helpers
  timeparse.rs       normalises source-reported post times; rejects implausible values
  db.rs              SQLite: schema, dedup/seen, candidates, budget, settings, IDF corpus
  pipeline.rs        ingest: classify → score → tier → draft-ahead → persist
  classify.rs        is-this-a-hiring-post filter (drops "open to work" posts)
  score.rs           structural signals + content signal; freshness-weighted priority()
  resume.rs          >>> tokenizer, IDF corpus, resume vector, similarity <<<
  draft.rs           TemplateDrafter (offline) + OllamaDrafter (local LLM); channel-shaped
  release.rs         >>> the tiered release engine <<<
  notify.rs          ntfy push, self-alert email, outbound application send, digest
  web.rs             axum dashboard: SSE live updates, radar view, settings, editable drafts
  sources/
    greenhouse.rs        public boards API — safe, real, no ban risk (start here)
    linkedin_guest.rs    guest jobs endpoint — no login, lower risk, HTML parsed
    linkedin_voyager.rs  authenticated content search — real hiring posts, FRAGILE
```

Flow: `crawl → dedup(URN) → classify → score → tier(drop if <floor) →
draft-ahead (strong+) → pool`. Then, on every tick and right after each crawl:
`expire → check budget → walk pool best-first under per-poster cap + adaptive
bar → fire → ntfy + email + dashboard SSE`.

On the **first** crawl of a source, everything it returns is ingested as
`backfilled`: scored and visible on the radar, but excluded from the release
pool, so history populates the board without firing a single notification. Only
genuinely-new posts after that can fire.

## Testing without hitting a live source

`RADAR_GREENHOUSE_BASE` overrides the board API host, so the whole pipeline can
be exercised against a fixture server. It logs a warning whenever it's set.

```bash
cargo test          # timeparse, salary detection, freshness decay, resume ranking
```

## Honest caveats

- **Finding 4 is still open**: a candidate is marked fired *before* dispatch, so
  if ntfy and SMTP both fail, a slot is burned and that post can never re-fire.
  Check `reviews/` for the current punch list.
- **LinkedIn Voyager is the fragile part.** The `query_id` and JSON shape change
  when LinkedIn ships. It's disabled by default; when you enable it, expect to
  grab a fresh `query_id` from DevTools and adjust `looks_like_post` in
  `linkedin_voyager.rs`. Authenticated scraping also carries real account-ban
  risk — use one dedicated account, keep polling gentle, keep the jitter on.
- **The guest endpoint returns formal listings**, not personal "we're hiring"
  posts, and its cards carry no description body — so they score on title and
  company alone. Voyager is what gets the people-posts.
- **Auto-send is email-only.** LinkedIn DMs are always drafted for you to send
  by hand; the dashboard gives you Copy draft + Open profile.
- A wrong Greenhouse board token is a silent 404, not an error. An empty radar
  and a misspelled token look identical.

## Reviews

`reviews/` is the project's punch list: one dated file per review pass, findings
struck off only when the fix is committed.
