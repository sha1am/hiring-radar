import type { Level, Source, Status } from '../types'
import { Heading } from '../ui'

/// The panel that answers "why is this empty".
///
/// An empty radar has several causes that look identical from the outside:
/// first crawl still running, source switched off, board tokens 404ing, or
/// everything scoring below the floor. Reading container logs to tell them
/// apart defeats the point of having a dashboard, so the state is on the page.

const BANNER: Record<Level, string> = {
  ok: 'bg-emerald-500/10 text-emerald-200 ring-emerald-500/20',
  info: 'bg-sky-500/10 text-sky-200 ring-sky-500/20',
  warn: 'bg-amber-500/10 text-amber-200 ring-amber-500/20',
  error: 'bg-rose-500/10 text-rose-200 ring-rose-500/20',
}

function since(ts: number | null): string {
  if (!ts) return 'never'
  const s = Math.max(0, Math.floor(Date.now() / 1000) - ts)
  if (s < 60) return `${s}s ago`
  if (s < 3600) return `${Math.floor(s / 60)}m ago`
  return `${Math.floor(s / 3600)}h ago`
}

function until(ts: number | null): string {
  if (!ts) return ''
  const s = ts - Math.floor(Date.now() / 1000)
  if (s <= 0) return 'due'
  if (s < 60) return `in ${s}s`
  if (s < 3600) return `in ${Math.floor(s / 60)}m`
  return `in ${Math.floor(s / 3600)}h`
}

/// Last cycle, in the terms that explain an empty board — and only the terms
/// that are non-zero. A row of "0 below floor · 0 wrong location · 0 not
/// hiring" is four numbers you have to read to learn nothing.
function counters(s: Source): string {
  const parts = [`${s.fetched} fetched`, `${s.stored} kept`]
  if (s.below_floor > 0) parts.push(`${s.below_floor} below floor`)
  if (s.wrong_location > 0) parts.push(`${s.wrong_location} wrong place`)
  if (s.wrong_stack > 0) parts.push(`${s.wrong_stack} wrong stack`)
  if (s.not_hiring > 0) parts.push(`${s.not_hiring} not hiring`)
  if (s.best_score > 0) parts.push(`best ${s.best_score}`)
  return parts.join(' · ')
}

function SourceCard({ s }: { s: Source }) {
  const dot = !s.enabled
    ? 'bg-slate-700'
    : s.last_error
      ? 'bg-rose-400'
      : s.running
        ? 'bg-sky-400 animate-pulse'
        : 'bg-emerald-400'

  const when = !s.enabled
    ? 'off'
    : s.running
      ? 'crawling now'
      : s.last_run
        ? `${since(s.last_run)}${s.next_run ? ` · next ${until(s.next_run)}` : ''}`
        : 'waiting for first crawl'

  const failures = s.notes.filter((n) => !n.ok)
  const ok = s.notes.filter((n) => n.ok)

  return (
    <div className="rounded-lg bg-slate-900/40 p-3 ring-1 ring-slate-800/80">
      <div className="flex items-center gap-2">
        <span className={`h-1.5 w-1.5 shrink-0 rounded-full ${dot}`} />
        <span className="text-sm text-slate-200">{s.label}</span>
        <span className="text-xs text-slate-600">{when}</span>
      </div>

      {s.enabled && s.last_run !== null && (
        <p className="mt-1 text-xs text-slate-500">{counters(s)}</p>
      )}

      {s.last_error && <p className="mt-2 text-xs text-rose-300/90">{s.last_error}</p>}

      {/* Failures first and in full — this is the line that identifies a
          mistyped board token, which is otherwise completely silent. The
          successes are folded away: forty "stripe: 12 jobs" lines are noise
          until the one that says 404 is buried among them. */}
      {failures.length > 0 && (
        <ul className="mt-2 space-y-1">
          {failures.map((n, i) => (
            <li key={i} className="text-xs leading-relaxed text-rose-300/90">
              {n.text}
            </li>
          ))}
        </ul>
      )}

      {ok.length > 0 && (
        <details className="mt-2">
          <summary className="cursor-pointer text-xs text-slate-600 hover:text-slate-400">
            {ok.length} target{ok.length === 1 ? '' : 's'} fine
          </summary>
          <ul className="mt-1 space-y-0.5">
            {ok.map((n, i) => (
              <li key={i} className="font-mono text-[11px] text-slate-600">
                {n.text}
              </li>
            ))}
          </ul>
        </details>
      )}
    </div>
  )
}

export function StatusPanel({ status }: { status: Status }) {
  return (
    <section className="space-y-4">
      <div className={`rounded-lg px-3 py-2.5 text-xs leading-relaxed ring-1 ${BANNER[status.level]}`}>
        {status.message}
      </div>

      <div className="space-y-3">
        <Heading
          right={
            <span className="text-xs text-slate-600">
              {status.rows_in_window} on the board · last {status.window_hours}h
            </span>
          }
        >
          Sources
        </Heading>

        {status.sources.length === 0 ? (
          <p className="text-xs text-slate-600">Starting up…</p>
        ) : (
          <div className="grid gap-2 sm:grid-cols-2">
            {status.sources.map((s) => (
              <SourceCard key={s.name} s={s} />
            ))}
          </div>
        )}
      </div>
    </section>
  )
}
