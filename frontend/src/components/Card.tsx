import { useEffect, useState } from 'react'
import type { Card as CardT } from '../types'
import { dismissCandidate, markApplied, saveDraft, sendCandidate } from '../api'

const TIER: Record<string, string> = {
  exceptional: 'bg-rose-500/15 text-rose-300 ring-rose-500/30',
  strong: 'bg-amber-500/15 text-amber-300 ring-amber-500/30',
  marginal: 'bg-slate-700/40 text-slate-400 ring-slate-600/40',
}

/// An actionable card: the post, the draft, and the three things you can do.
///
/// The draft is editable before it goes anywhere, and nothing is ever sent
/// without a click. That is the whole design of the outbox — a tool that mails
/// strangers on your behalf while you sleep is one bad score away from being
/// mortifying.
export function Card({
  card,
  onChanged,
}: {
  card: CardT
  onChanged: () => void
}) {
  const [subject, setSubject] = useState(card.draft_subject ?? '')
  const [body, setBody] = useState(card.draft_body ?? '')
  const [busy, setBusy] = useState(false)
  const [note, setNote] = useState<{ text: string; ok: boolean } | null>(null)

  // A refetch (SSE tick, filter change) hands us a new draft from the server.
  // Adopt it only when this card isn't mid-edit, or a background refresh eats
  // what you were typing.
  const [dirty, setDirty] = useState(false)
  useEffect(() => {
    if (dirty) return
    setSubject(card.draft_subject ?? '')
    setBody(card.draft_body ?? '')
  }, [card.draft_subject, card.draft_body, dirty])

  const run = async (fn: () => Promise<unknown>, ok: string) => {
    setBusy(true)
    setNote(null)
    try {
      await fn()
      setNote({ text: ok, ok: true })
      setDirty(false)
      onChanged()
    } catch (e) {
      setNote({ text: e instanceof Error ? e.message : 'failed', ok: false })
    } finally {
      setBusy(false)
    }
  }

  return (
    <article className="rounded-lg bg-slate-900/50 p-3 ring-1 ring-slate-800">
      <div className="flex items-start gap-2">
        <div className="min-w-0 flex-1">
          <a
            href={card.url}
            target="_blank"
            rel="noreferrer noopener"
            className="block font-medium text-slate-100 hover:text-sky-300"
          >
            {card.title}
          </a>
          <p className="truncate text-xs text-slate-500">
            {card.company}
            {card.location && ` · ${card.location}`} · {card.source_label} · {card.age}
          </p>
        </div>
        <span
          className={`shrink-0 rounded-full px-2 py-0.5 font-mono text-xs ring-1 ${
            TIER[card.tier] ?? TIER.marginal
          }`}
          title={`${card.tier} · live rank ${card.priority}`}
        >
          {card.score}
        </span>
      </div>

      <FactLine card={card} className="mt-2" />

      {card.tags.length > 0 && (
        <div className="mt-1.5 flex flex-wrap gap-1">
          {card.tags.map((t) => (
            <span key={t} className="rounded bg-slate-800 px-1 text-[10px] text-slate-400">
              {t}
            </span>
          ))}
        </div>
      )}

      {card.why && (
        <p className="mt-2 text-xs text-slate-500">
          Matched your resume on <span className="text-slate-400">{card.why}</span>
        </p>
      )}

      {card.excerpt && (
        <p className="mt-2 line-clamp-3 text-xs leading-relaxed text-slate-400">{card.excerpt}</p>
      )}

      <div className="mt-3 space-y-2">
        {card.apply_kind === 'email' && (
          <input
            value={subject}
            onChange={(e) => {
              setSubject(e.target.value)
              setDirty(true)
            }}
            placeholder="Subject"
            className="w-full rounded-md border border-slate-700 bg-slate-950/60 px-2 py-1 text-xs text-slate-200"
          />
        )}
        <textarea
          value={body}
          onChange={(e) => {
            setBody(e.target.value)
            setDirty(true)
          }}
          rows={5}
          className="w-full rounded-md border border-slate-700 bg-slate-950/60 px-2 py-1.5 font-mono text-xs leading-relaxed text-slate-200"
        />

        <div className="flex flex-wrap items-center gap-2">
          {card.apply_kind === 'email' ? (
            <button
              disabled={busy}
              onClick={() => run(() => sendCandidate(card.id, subject, body), 'sent')}
              className="rounded-md bg-emerald-500/90 px-3 py-1.5 text-xs font-medium text-slate-900 hover:bg-emerald-400 disabled:opacity-50"
            >
              Send → {card.apply_target}
            </button>
          ) : (
            <>
              {/* No submit button: there is nothing to submit to. A DM has to be
                  pasted into LinkedIn, an external listing has its own form. So
                  the draft is saved for copying, and you mark it applied when
                  you have actually done it. */}
              <button
                disabled={busy}
                onClick={() => run(() => saveDraft(card.id, subject, body), 'saved')}
                className="rounded-md bg-slate-700 px-3 py-1.5 text-xs text-slate-100 hover:bg-slate-600 disabled:opacity-50"
              >
                Save draft
              </button>
              {card.apply_target && (
                <a
                  href={card.apply_target}
                  target="_blank"
                  rel="noreferrer noopener"
                  className="rounded-md bg-sky-500/80 px-3 py-1.5 text-xs font-medium text-slate-900 hover:bg-sky-400"
                >
                  {card.apply_kind === 'dm' ? 'Open profile' : 'Open listing'}
                </a>
              )}
            </>
          )}

          <button
            disabled={busy}
            onClick={() => run(() => markApplied(card.id), 'marked applied')}
            className="text-xs text-slate-400 hover:text-slate-200 disabled:opacity-50"
          >
            Applied
          </button>
          <button
            disabled={busy}
            onClick={() => run(() => dismissCandidate(card.id), 'dismissed')}
            className="text-xs text-slate-500 hover:text-rose-300 disabled:opacity-50"
          >
            Dismiss
          </button>

          {note && (
            <span className={`text-xs ${note.ok ? 'text-emerald-400' : 'text-rose-400'}`}>
              {note.text}
            </span>
          )}
        </div>
      </div>
    </article>
  )
}

/// The one-line summary of what a posting *is* — role, level, years, where.
/// Only what the posting actually stated; a missing piece is left out rather
/// than guessed at.
function FactLine({ card, className = '' }: { card: CardT; className?: string }) {
  const bits = [card.role, card.level, card.years, card.work_mode, card.employment].filter(Boolean)
  // The region is on the row already via the location string; showing the
  // bucket too would be saying "Bengaluru, India" and then "india".
  if (bits.length === 0) return null
  return (
    <span className={`flex flex-wrap gap-1 ${className}`}>
      {bits.map((b) => (
        <span
          key={b}
          className="rounded bg-violet-500/10 px-1 text-[10px] text-violet-300/90 ring-1 ring-violet-500/20"
        >
          {b}
        </span>
      ))}
    </span>
  )
}

/// The compact row used for the radar list — everything crawled, not just what
/// fired. Deliberately not a Card: a hundred of those is a wall, and the radar
/// is for scanning.
export function RadarRow({ card }: { card: CardT }) {
  return (
    <li>
      <a
        href={card.url}
        target="_blank"
        rel="noreferrer noopener"
        className="flex items-start gap-3 rounded-md px-2 py-1.5 hover:bg-slate-900/60"
      >
        <span
          className={`mt-0.5 shrink-0 rounded px-1.5 font-mono text-xs ring-1 ${
            TIER[card.tier] ?? TIER.marginal
          }`}
        >
          {card.score}
        </span>
        <span className="min-w-0 flex-1">
          <span className="block truncate text-slate-200">{card.title}</span>
          <span className="block truncate text-xs text-slate-500">
            {card.company}
            {card.location && ` · ${card.location}`} · {card.source_label} · {card.age}
            {card.status !== 'queued' && ` · ${card.status}`}
          </span>
          <FactLine card={card} className="mt-0.5" />
          {card.tags.length > 0 && (
            <span className="mt-0.5 flex flex-wrap gap-1">
              {card.tags.slice(0, 5).map((t) => (
                <span key={t} className="rounded bg-slate-800 px-1 text-[10px] text-slate-400">
                  {t}
                </span>
              ))}
            </span>
          )}
        </span>
      </a>
    </li>
  )
}
