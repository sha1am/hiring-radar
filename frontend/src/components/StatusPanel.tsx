import { useState } from 'react'
import type { Level, Source, Status } from '../types'

/// The panel that answers "why is this empty".
///
/// An empty radar has several causes that look identical from the outside:
/// first crawl still running, source switched off, board tokens 404ing, or
/// everything scoring below the floor. Reading container logs to tell them
/// apart defeats the point of having a dashboard, so the state is on the page.

const BANNER: Record<Level, { ring: string; icon: string }> = {
  ok: { ring: 'bg-emerald-500/10 ring-emerald-500/30 text-emerald-300', icon: '✓' },
  info: { ring: 'bg-sky-500/10 ring-sky-500/30 text-sky-300', icon: '…' },
  warn: { ring: 'bg-amber-500/10 ring-amber-500/30 text-amber-300', icon: '!' },
  error: { ring: 'bg-rose-500/10 ring-rose-500/30 text-rose-300', icon: '×' },
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

function SourceRow({ s }: { s: Source }) {
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

  return (
    <li className="py-1.5">
      <div className="flex items-center gap-2">
        <span className={`h-1.5 w-1.5 shrink-0 rounded-full ${dot}`} />
        <span className="text-slate-300">{s.label}</span>
        <span className="text-xs text-slate-500">{when}</span>
      </div>

      {s.enabled && s.last_run !== null && (
        <p className="ml-3.5 text-xs text-slate-500">
          {s.fetched} fetched · {s.new_posts} new · {s.stored} kept
          {s.below_floor > 0 && ` · ${s.below_floor} below floor`}
          {s.wrong_location > 0 && ` · ${s.wrong_location} wrong location`}
          {s.wrong_stack > 0 && ` · ${s.wrong_stack} wrong stack`}
          {s.not_hiring > 0 && ` · ${s.not_hiring} not hiring`}
          {s.best_score > 0 && ` · best ${s.best_score}`}
        </p>
      )}

      {/* Per-target notes are the line that identifies a mistyped board token,
          which is otherwise a completely silent failure. */}
      {s.notes.length > 0 && (
        <ul className="ml-3.5 mt-0.5 space-y-0.5">
          {s.notes.map((n, i) => (
            <li key={i} className={`text-xs ${n.ok ? 'text-slate-600' : 'text-rose-400/90'}`}>
              {n.ok ? '·' : '×'} {n.text}
            </li>
          ))}
        </ul>
      )}

      {s.last_error && <p className="ml-3.5 text-xs text-rose-400/90">× {s.last_error}</p>}
    </li>
  )
}

export function StatusPanel({ status }: { status: Status }) {
  // Expanded by default: this is its own tab now, and you opened it to see
  // exactly what the collapsed version was hiding.
  const [open, setOpen] = useState(true)
  const b = BANNER[status.level]

  return (
    <section className="space-y-2">
      <div className={`flex items-start gap-2 rounded-lg px-3 py-2 text-sm ring-1 ${b.ring}`}>
        <span aria-hidden className="mt-px font-mono">
          {b.icon}
        </span>
        <p className="flex-1">{status.message}</p>
        <button
          onClick={() => setOpen(!open)}
          className="shrink-0 text-xs opacity-70 hover:opacity-100"
        >
          {open ? 'hide sources' : 'sources'}
        </button>
      </div>

      {open && (
        <ul className="divide-y divide-slate-800/70 rounded-lg bg-slate-900/40 px-3 py-1 text-sm ring-1 ring-slate-800">
          {status.sources.length === 0 ? (
            <li className="py-2 text-xs text-slate-500">Starting up…</li>
          ) : (
            status.sources.map((s) => <SourceRow key={s.name} s={s} />)
          )}
        </ul>
      )}
    </section>
  )
}
