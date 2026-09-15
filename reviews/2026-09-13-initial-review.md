# 2026-09-13 — initial review

First full read of the generated codebase, at the commit where `hiring-radar.zip`
was extracted to the repo root. Reviewed every file under `src/`, plus
`Cargo.toml` and `config.example.toml`.

Status is updated as fixes land. A finding is only struck off once the fix is
committed.

| # | Finding | Status | Commit |
|---|---------|--------|--------|
| 0 | Did not compile — `r#"…"#` raw string in `card()` closed early on `hx-target="#queue"` | fixed | `4160a69` |
| 1 | Freshness half-life never ran; stored `priority` == `score` for every row | fixed | `09fa0ff` |
| 2 | `linkedin_guest` could never reach the tier that fires | fixed | `dea9184` |
| 3 | `per_poster_cap > 1` unreachable | fixed | `09fa0ff` |
| 4 | Fired ≠ delivered — a failed send still burns a budget slot | fixed | `3b23fdb` |
| 5 | Adaptive bar is wall-clock; the budget is rolling | fixed | `3b23fdb` |
| 6 | `/candidate/:id/draft` unreachable; DM/external edits lost | fixed | `82d736a` |
| 7 | Substring matching over the full body misfires both ways | fixed | `dea9184` |
| 8 | SQLite without WAL, five writers | fixed | `82d736a` |
| 9 | `detect_salary` read any 6-digit number as pay | fixed | `dea9184` |

## Fixed in pass 3

**4. Fired ≠ delivered.** The claim still comes first — it is what makes two
concurrent ticks safe — but dispatch now reports which channels worked, and a
send that delivered nothing calls `unfire`: the notification row is deleted, the
candidate goes back to 'scored' for the next tick to retry, and the failure is
counted. Three failures park it as 'undeliverable', which shows on the
dashboard, because a wrong SMTP password is not transient. The notifications
table records what was delivered ('pending' → 'ntfy' / 'email' / 'ntfy+email')
rather than what was attempted. Separately, an ntfy POST answered 403 and a POST
to an empty topic are both errors now instead of silent successes.

**5. Adaptive bar.** Now a function of `remaining / per_hour_cap` and nothing
else: strict on the last slot, relaxed when the budget is untouched, recomputed
inside the release loop so a pass that fires three raises its own bar. The
settings labels were renamed with it — "Bar at :00" described behaviour that no
longer exists.

## Minor — remaining

- Jitter is `0..=jitter` (additive only); README says "+/-".
- History strip only renders on full page load.
- `tower-http` is in `Cargo.toml` but unused in `src/`.
- `scorer` is constructed in `pipeline.rs` while `drafter` lives in `AppState`.
- No auth on the dashboard. The container binds `0.0.0.0` and compose publishes to
  `127.0.0.1` only — that loopback binding is now the *only* thing protecting a
  "send email as Shadab" button. Worth real auth before this is ever exposed.
- `gitlab` is not a Greenhouse board — that example token 404s.
- A new `AsyncSmtpTransport` (TLS handshake + login) is built per email.

## Minor — fixed

- `javascript:` hrefs from scraped payloads (`82d736a`, scheme-checked).
- Release engine idled one full tick at startup (`82d736a`).
- Unbounded candidate growth (`82d736a`, 6h prune timer).
- `thiserror` declared but unused (`09fa0ff`).
- Apostrophes unescaped in rendered HTML (`82d736a`).

## Environment notes

- `cargo` is not installed on the MacBook, and the sandboxed Linux VM there cannot
  reach `sh.rustup.rs` or `static.rust-lang.org` (egress policy), so a toolchain
  cannot be installed from a Claude session. This is why the project is
  containerised.
- Neither sandbox can reach `boards-api.greenhouse.io`, `ntfy.sh` or
  `linkedin.com`, and neither has a Docker daemon. **A live end-to-end run can
  only happen on the macOS host.** Everything verified so far was verified against
  a local fixture server via `RADAR_GREENHOUSE_BASE`.
- Git in that sandbox cannot unlink its own lock files. They are renamed aside
  before each call, which leaves `.git/*.lock.junk*` files behind — harmless, but
  worth sweeping.

## Net

The release engine — tiers, rolling budget, rollover, exactly-once via
UNIQUE(candidate_id) — was sound from the start and remains the interesting part.
The two headline behaviours that didn't actually run (freshness-weighted priority,
per-poster cap > 1) now do. What remains open is finding 4, which is the one that
can still cost you a real alert.
