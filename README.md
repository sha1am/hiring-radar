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

Both the volume and the container are named after the directory this repo sits
in, so **renaming or moving the directory starts you on a fresh, empty volume**.
The old one is still there under the old name (`docker volume ls`) — nothing is
lost, but the new instance bootstraps from scratch. That is usually what you
want after a rename; if not, copy the old volume across before starting.

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

Upload a PDF (or paste the text) in **Settings → Resume**. It is read once into
a profile — years, level, discipline, stack — and every posting is scored
against that profile instead of against a keyword list you maintain by hand.

Reading the resume is the part that is easy to get wrong, because a resume is
two documents in one: a record of what you did, and an index of words you want a
search to hit. So the reader separates them. Roles and level come from the
experience and projects sections only — a line reading `Frontend: React, HTML5,
CSS3` is a vocabulary claim, not a job, and reading it as one had every frontend
posting scoring as though frontend were the work. The stack comes from the whole
document, because the skills list is exactly where a resume names its
technologies and is the one section that means what it says.

Experience comes from the dates in the jobs, merged so a project running
alongside a job is not double counted and gaps between jobs are not counted at
all — and not from the degree four lines below, which would have added four
years of professional experience to every new graduate. Level follows from the
years; a level claimed in the work section can only raise it, never lower it.
("St. Michael's Sr. Sec. School" is not a claim to be senior. It was making
one.)

Disciplines are ranked by evidence rather than by presence. One mention of
"full-stack" in a project title is not the same claim as backend work described
across nine bullets, and a secondary discipline counts for less when a posting
is scored.

Settings shows exactly what was read — years, level, discipline, the technology
list — because every score on the board is downstream of it, and a profile you
cannot see is one you cannot correct.

## Scoring: an ATS, pointed the other way

`src/ats.rs` asks what a real applicant tracking system asks — does this person
meet the requirements, and which ones do they miss — and answers it in the
direction that is useful to the candidate. Five dimensions, weighted to a
hundred:

| dimension | weight | the question |
|-----------|--------|--------------|
| stack     | 35     | do you write what they need written? |
| years     | 20     | are you experienced enough? |
| role      | 20     | is this the kind of engineering you do? |
| level     | 15     | is it pitched at your level? |
| location  | 10     | can you take it? |

The weighted sum is then scaled by a relevance factor built from the stack and
role fits. A flat sum let years, level and location — three things true of you
wherever you apply — carry a frontend posting to 49 for a backend engineer, and
nobody is hired for a frontend role because they live in the right city.

Languages count double inside the stack: swapping Postgres for MySQL is a
Monday, swapping Go for Scala is a quarter. Years are a slope rather than a gate
— one short is 0.8, two is 0.55 — because "5+ years" has never once meant a
person with four was turned away. Roles have neighbours: SRE and devops are 0.9
of each other, backend and frontend 0.2.

Out of it comes a verdict — apply, stretch, reach, skip — which is allowed to
disagree with the number: a posting can score 91 and still be a stretch because
you are two years short. Every row can show its working, dimension by dimension,
with what you meet and what you are missing. Aggregated across the board, the
missing list answers "what should I learn next", which is a better question than
"what should I apply to".

Scoring itself is arithmetic — no call, no waiting — so the number on the card is
the number the release engine acted on and you can reproduce it by hand. What a
model changes is what goes *into* it, plus one dimension of its own:

| dimension | weight | the question |
|-----------|--------|--------------|
| judgement | 15     | what does a model make of you against this posting? |

Added on top of the hundred rather than carved out of it, and the total is
divided by the weight actually present. Two reasons. With no model configured,
every number on the board is exactly what it was — turning one on is the only
thing that moves scores, not an invisible rebalancing of the five dimensions
that were already there. And with one, fifteen out of a hundred and fifteen is
about right for an opinion: enough to separate two postings the dimensions score
identically, never enough to carry a job you cannot do past a stack score of
zero. It is a dimension like any other, so it appears in the breakdown with the
model's own sentence saying why.

Postings from other departments are ruled out before any of this: recruiting,
sales, clinical, operations and the rest arrive on the same company boards as
the engineering roles, name no technology and no discipline, and would otherwise
collect the benefit of the doubt on both and pass on years, level and location
alone. A title carrying an engineering word as a whole word is engineering
whatever else it says, which is what keeps "Software Engineer, Sales Platform"
and drops "Engineering Recruiter".

## Letting a model read the postings

Optional, and off by default. With `[llm] provider = "none"` the heuristics do
all the reading: the board still works, the filters still fill, the scores are
just blunter.

With a model configured, every posting gets one call — in the background, never
on the crawl path, because inference takes seconds per posting and a crawl that
waits for it would turn a ninety-second cycle into an hour and miss the posts it
exists to catch. What comes back is stored in full:

- **must-have versus nice-to-have.** The single biggest thing a model adds. "5+
  years of Go" and "exposure to Kubernetes a plus" were worth exactly the same
  before, because a flat list of technologies cannot express the difference —
  and the difference is most of what a requirement means. A preferred
  technology is now worth about a third of a required one and never appears in
  the list of things you are missing: a list of things to go and learn should
  not be padded with things the posting called a bonus.
- **what the job involves, and what business it is in** — responsibilities and a
  domain, both closed-vocabulary where it matters.
- **pay, when it is stated**, formatted the way it was quoted: lakhs for rupees,
  because every Indian posting says LPA and "3500k" is a number nobody says out
  loud.
- **visa sponsorship**, and **red flags** — an unpaid task, equity instead of
  salary, ten years wanted for a mid-level title.
- **a one-sentence summary**, which sits above the assessment on the card,
  because "what is this job" comes before "how well does it suit you".
- **its own 0-100 read of the fit**, with the sentence that decided it.
- **the reply exactly as it arrived.** A schema is a guess about what will
  matter later, and this is the only copy of what was actually said: re-reading
  a stored reply is free, re-asking is a call per posting.

The candidate goes into the prompt alongside the posting, which is what makes
the call worth paying for — "does this want five years of Kafka" is a question
about the text, "is this worth your evening" is a question about both.

Configure it in `config.toml`:

```toml
[llm]
provider = "openai"                     # "none" | "openai" | "ollama"
base_url = "https://api.openai.com/v1"  # or OpenRouter, Groq, Ollama's /v1
model    = "gpt-5.4-mini"
```

and put `OPENAI_API_KEY` in `.env` — a key in a mounted config file is a key in
your shell history and your backups. Everything hosted speaks the same
chat-completions shape, so pointing `base_url` somewhere else is the whole
migration. JSON mode rather than a strict schema, because the strict one is not
understood everywhere and the reply has to be validated in Rust regardless: a
model *will* occasionally return a string where a number belongs, and one bad
reply must not poison a batch.

The Status tab shows which model is reading, how many postings it has got
through, how many are queued and how many tokens it has spent. That last number
is there because this is the only part of the system billed per posting, and a
counter you can see is the difference between an experiment and a surprise.

## Watching for a technology

**Settings → Tell me the moment you see** is a list of technologies — `go` by
default. A posting naming one of them skips the score floor, skips the settling
window and skips the release bar: it lands on the board and goes out the moment
it is found, whatever else it scored. The hourly cap still holds, so this
changes what is worth an interruption, not how many interruptions there are.

It is a standing alert, not a filter. "Tell me about every Go job" and "hide the
jobs that are not Go" are different requests — the second is `stack` plus
`stack policy` — and answering the first with the second gets you a quiet board
and no idea what you missed. Rows that came in this way carry an amber dot.

## The outbox

Every post scoring at or above the **outbox bar** (Settings, default 70) gets a
ready-to-edit draft and sits in `/outbox` until you send, apply or dismiss it.

This is deliberately independent of the alert budget. The 4/hour cap limits how
often your phone buzzes; it should not limit how much you can work through. A
post that scored 88 on a busy morning is no less worth applying to for having
missed a notification slot, and tying the two together is how a job-hunting tool
quietly loses you the job.

Backfilled posts are stored without a draft — drafting hundreds during the first
crawl would stall it, badly so with a local LLM configured — so the outbox fills
them in for what is actually on screen and persists them, which happens once.

## Filtering the board

The radar has a filter bar: free text over title, company, place and the matched
resume terms, plus minimum score, tier, source and status. They compose, and the
window buttons carry them, so widening from 24h to 7d narrows the same search
rather than silently resetting it.

Filtering runs in SQL across the whole window, not over the rendered list. The
query is capped at a few hundred rows, so filtering after the cap would search
only the newest slice and quietly miss matches further back — the kind of bug
you would never notice, because a filter that finds nothing looks exactly like a
filter that works.

A filter matching nothing says so ("No match among the 328 in this window")
rather than showing the same empty state as a board with nothing on it.

Technology chips sit under the filter bar: Go, Kafka, Postgres and so on,
extracted from each post at ingest and offered with counts. Selecting several
ORs them, because chips are a net for scanning rather than a sieve — picking Go
and Rust should widen the board to both. Each chip carries the rest of the
selection, so clicking toggles one rather than replacing everything.

The extractor is a curated list, for the same reason the gazetteer is: the set
of things a backend engineer filters on is small, stable, and full of aliases
nothing would guess (`k8s` is Kubernetes, `psql` is Postgres). Bare "go" needs
corroboration from something Go-flavoured before it counts, or every
"ready to go" post gets tagged.

htmx is served from the binary at `/assets/htmx.min.js`, not a CDN. Every
interactive part of this dashboard is htmx — the filters, saving settings,
dismiss and apply — so loading it from unpkg means that when you are offline, or
the CDN has a bad day, a self-hosted tool running in a container on your own
machine silently loses all of its buttons. Tailwind is still a CDN script; if it
fails the page is ugly but works.

## Modes

Settings → Sources has three presets over the individual toggles:

| Mode | What it watches | Needs |
|------|-----------------|-------|
| **Job boards only** | Greenhouse boards | nothing — no login, no ban risk |
| **LinkedIn posts only** | hashtag searches of the feed for "we're hiring" posts | `li_at` cookie + a live queryId |
| **Everything** | all three sources | as above |

The mode owns the toggles: switching to a preset overrules whatever the
individual checkboxes said, so the UI can never claim one thing while the crawl
loops do another.

### LinkedIn posts mode

This is the mode the project was originally about — actual posts from people
hiring, not formal listings. Setup:

1. Put your cookie in `.env`: `LI_AT=...` (and `LI_JSESSIONID=...`). These are
   secrets, so they stay out of the web form and out of git. **Changing `.env`
   needs a container restart** — the environment is read once at boot.
2. Open a LinkedIn content search with DevTools → Network, find the
   `voyager/api/graphql` request, and copy its `queryId`. Paste it into
   Settings → Sources. It is not a secret — just a constant that breaks whenever
   LinkedIn ships — so it lives in settings and needs no rebuild.
3. Set your searches, one per line. Hashtags work best: `#hiring`, `#hiringnow`.
   Each runs as its own search rather than one OR'd blob, which returns a better
   mix.

### Covering a window

"Every #hiring post from the last 24 hours" needs two things that one request
does not give you.

**Real post times.** Voyager reports no timestamp, so a 24-hour window would
otherwise measure when *we crawled*, not when anything was posted — a week-old
post looks brand new the moment it is first seen. LinkedIn activity ids are
Snowflake-style, with creation time in the high 41 bits, so the time comes out
of the URN with no extra request. The bit layout is undocumented, so the result
goes through the same plausibility gate as every other timestamp: if it is
wrong the answer lands decades away, gets rejected, and detection time is used
instead.

**Pagination.** One request returns one page, which on a busy hashtag can be
under an hour. Each search now walks back page by page until it passes the
window, sorted by date so walking pages walks backwards in time.

That walk happens **once**. Repeating it every crawl would be three hashtags
times eight pages every 90 seconds — roughly a thousand authenticated requests
an hour against an API that bans accounts for much less. Once the window is
filled, only new posts matter and new posts are on page one, so later crawls
read `Pages when polling` (default 2) instead. Both are in Settings, along with
`Collect posts from the last (h)`.

If a search hits its page limit before covering the window, the status panel
says so — `#hiring: 100 posts in last 24h (5 pages, reached 16.5h back), page
limit hit` — rather than quietly giving you a partial window.

A feed post has no title field, so its "title" is just the first line. Matching
job titles against that is meaningless, so for these posts the title budget is
redistributed into the content score, where the post's own text can earn it.
Without that, every feed post would forfeit 40 points and sit below any sane
floor — the mode would look broken rather than empty.

When it breaks — and it will, because Voyager is LinkedIn's private API — the
status panel says how. A 200 that yields no posts reports the payload's actual
shape, so you can correct the field names in `looks_like_post` instead of
guessing. 401/403 means the cookie expired; 400 means the queryId or variables
were rejected.

## Location

`locations` used to be worth ten points out of a hundred, which meant it never
actually decided anything: a San Francisco role with a strong resume match
cleared the floor and alerted anyway. Settings → **Location matching** now
chooses how hard the list bites:

- **Only these locations** — anything elsewhere is discarded outright, like a
  dealbreaker. This is what "only send me India jobs" has to mean.
- **Prefer these locations** — the old behaviour: adds points, never decides.
- **Ignore location** — score on the work alone.

Place names are canonicalised on both sides, which matters more than it sounds:
you write "Gurugram", the post says "Gurgaon"; you write "India", the post says
"Bengaluru". Plain substring matching misses both, and under a hard filter it
discards exactly the jobs you set the filter up to keep. A country therefore
matches every city inside it, aliases resolve to one canonical name, and
matching is whole-word so "Pune" does not match "Puneet".

Feed posts state no location at all, so one is inferred from the post text
before scoring — otherwise an India-only filter would silently pass every
LinkedIn post for want of a value. When nothing can be determined, that is
treated as its own case rather than as a miss: **Keep posts whose location I
can't determine** decides it, because rejecting unknowns is defensible but does
throw away real matches.

Posts dropped this way are counted separately on the status panel
("3 wrong location") rather than vanishing into the score-floor number, so
turning the filter on never looks like the profile suddenly matching nothing.

## Settings

`/settings` edits, live, with no restart:

- **Matching** — resume, years of experience, titles, keywords, locations,
  remote-ok, seniority, languages you write + stack policy, the instant list,
  dealbreakers, min salary.
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

## GitHub Actions Deployment

This project includes comprehensive GitHub Actions workflows for automated CI/CD:

### Automated Workflows

- **CI/CD Pipeline** (`.github/workflows/ci-cd.yml`)
  - Builds and tests Rust code with clippy and formatting checks
  - Builds and pushes Docker images to GitHub Container Registry (GHCR)
  - Deploys React frontend to GitHub Pages
  - Creates GitHub releases on version tags
  - Runs on every push to main/develop, pull requests, and version tags

- **Maintenance** (`.github/workflows/maintenance.yml`)
  - Nightly builds to ensure code stays working
  - Security audits for dependencies
  - Dependency checks for outdated packages
  - Runs daily at 2 AM UTC

- **Automated Release** (`.github/workflows/release.yml`)
  - Manual workflow to create versioned releases
  - Creates semantic version tags (e.g., v1.2.3)
  - Generates GitHub releases with changelog

### Setting Up GitHub Pages

1. Go to your repository Settings → Pages
2. Set Source to "GitHub Actions"
3. The frontend will be automatically deployed on push to main

### Creating a Release

Run the "Automated Release" workflow manually with a version number:
- Navigate to Actions → Automated Release → Run workflow
- Enter version in semantic format (e.g., 1.2.3)
- This creates a Git tag and GitHub release

### Docker Images

Images are automatically built and pushed to GHCR:
```bash
docker pull ghcr.io/yourusername/hiring-radar:latest
docker pull ghcr.io/yourusername/hiring-radar:v1.2.3
```

### Required GitHub Settings

- Enable GitHub Actions in repository settings
- Enable GitHub Pages for frontend deployment
- Ensure repository has appropriate permissions for workflows

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
