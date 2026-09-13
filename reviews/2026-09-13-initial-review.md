# 2026-09-13 — initial review

First full read of the generated codebase, at the commit where `hiring-radar.zip`
was extracted to the repo root. Reviewed every file under `src/`, plus
`Cargo.toml` and `config.example.toml`.

Status column is updated as fixes land. `open` = not yet addressed.

## Blocking

| # | Finding | Status |
|---|---------|--------|
| 0 | **The project did not compile.** `card()` in `web.rs` used an `r#"…"#` raw string whose body contains `hx-target="#queue"` — the `"#` closes the literal. 8 parse errors. Promoted to `r##"…"##`. | fixed |

## High

**1. The freshness half-life never does anything.**
`priority()` is called exactly once (`pipeline.rs:24`) as
`priority(score, post.posted_at, detected, detected)` — `now` and `detected_at` are
the same value. Every source sets `posted_at: None`, so `age_hours = 0`,
`freshness = 1.0`, and `priority == score` for every candidate ever stored. The
column is never recomputed; `db::eligible` orders by the frozen value. An
11-hour-old rollover post therefore outranks a brand-new post with a 1-point-lower
score — the exact inversion the README says the design prevents. `cargo check`
corroborates: `Candidate.priority` is flagged "never read" in Rust; it only ever
appears in SQL `ORDER BY`.
*Fix:* compute priority at selection time in `release::run`, or recompute the
column each tick.
**Status: open**

**2. `linkedin_guest` can never reach the tier that fires.**
`linkedin_guest.rs:116` sets `body: String::new()`, so `haystack()` is title +
company only. Keyword coverage (30 of 100 points) scores ~0/8. Realistic ceiling:
45 (title) + 10 (location) + 15 (seniority) ≈ 70, under `strong_min = 75`.
Everything from the always-on LinkedIn source lands marginal → digest only.
`exceptional_min = 92` is unreachable from any source without near-total keyword
coverage.
**Status: open**

**3. `per_poster_cap > 1` is unreachable.**
`release.rs:50` short-circuits on `chosen_companies.contains(&cand.company)` before
the `poster_prev >= per_poster_cap` check, capping every company at 1 per pass
regardless of config. Count per-company within the pass and compare to the cap.
**Status: open**

**4. Fired ≠ delivered — a broken SMTP config burns the whole budget silently.**
`mark_fired` writes the notification row (which *is* the budget) before dispatch,
and both `push_ntfy` and `email_self` failures are only `tracing::warn!`. With
`SMTP_PASSWORD` unset (`Config::load` merely warns), 4 candidates/hour are marked
notified with nothing delivered, and UNIQUE(candidate_id) means they can never
re-fire.
**Status: open**

## Medium

**5. Adaptive bar is wall-clock; the budget is rolling.**
`adaptive_bar` interpolates on `now() % 3600` (minutes past the top of the UTC
hour) while `budget_used` counts a trailing 60 minutes. The config comment
("required score at :00 with all 4 slots free") implies budget-awareness that does
not exist — the function never reads `remaining`. Drive it off
`remaining / per_hour_cap`.
**Status: open**

**6. Dead route — edited drafts are lost for DM/external posts.**
`/candidate/:id/draft` (`save_draft`) is registered but never referenced from any
rendered HTML. Only the email card has a submit button, and only `send` persists
the textarea.
**Status: open**

**7. Substring matching over the full body misfires both ways.**
`score.rs:20` zeroes a post if any dealbreaker appears anywhere in
title+company+body. On a full Greenhouse JD, "php" in a nice-to-have line kills a
perfect Go role. Inverted for keywords: `"go"` matches *Mongo, Django, Algolia,
category*. Word-boundary matching fixes both.
**Status: open**

**8. SQLite without WAL, five writers.**
`db::connect` sets `busy_timeout(5s)` but not `journal_mode = WAL`. N crawl loops +
the 30s release ticker + web handlers write through a 5-connection pool; in
rollback-journal mode writers block readers too.
**Status: open**

**9. `detect_salary` reads any 6-digit number as pay.**
"serving 1000000 requests/day" parses as a salary → silent −30 on exactly the
infra-heavy posts worth wanting. Only active when `min_salary` is set.
**Status: open**

## Minor

- Jitter is `0..=jitter` (additive only); README says "+/-".
- `release::run` isn't called once at startup — up to 30s dead on boot.
- History strip only renders on full page load; htmx swaps `#queue` alone.
- `thiserror` and `tower-http` are in `Cargo.toml` but unused in `src/`.
- `scorer` is hardcoded in `pipeline.rs` while `drafter` lives in `AppState` —
  asymmetric for two things the README calls "both traits".
- No auth on the dashboard. Fine on 127.0.0.1; a send-email-as-you hole the moment
  `bind` changes — which containerisation forces (see below).
- `esc()` doesn't block a `javascript:` href, and post URLs are attacker-influenced.
- `gitlab` is not a Greenhouse board — that example token 404s.
- `seen` table is never pruned.
- A new `AsyncSmtpTransport` (TLS handshake + login) is built per email.
- The DB path is hardcoded to `sqlite:hiring.db?mode=rwc` — blocks putting state on
  a mounted volume.

## Environment notes

- `cargo` is not installed on the MacBook, and the sandboxed Linux VM there cannot
  reach `sh.rustup.rs` or `static.rust-lang.org` (egress policy), so a toolchain
  cannot be installed from this session. Containerising is the way around it.
- Neither sandbox can reach `boards-api.greenhouse.io`, `ntfy.sh` or
  `linkedin.com`, so a live end-to-end run has to happen on the host, outside the
  sandbox — i.e. by running the container.

## Net

The release engine — tiers, rolling budget, rollover, exactly-once via
UNIQUE(candidate_id) — is sound and is the interesting part. What's broken is that
two of the three headline behaviours (freshness-weighted priority, per-poster cap
>1) don't actually run, and the one LinkedIn source that works out of the box can't
clear the bar to fire.
