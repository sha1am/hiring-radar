import { useEffect, useState } from 'react'
import type { Card as CardT } from '../types'
import { dismissCandidate, markApplied, saveDraft, sendCandidate } from '../api'
import { Breakdown, Button, Dotted, Score, TIER_ACCENT, VerdictBadge, inputClass, rankOf } from '../ui'

/// What a posting *is*, as a phrase rather than a row of chips.
///
/// "senior backend · 5+ yrs · hybrid" reads; five little pills of five
/// different colours do not. Chips are for clicking — they belong in the filter
/// bar — and using the same treatment for things you can't click was most of
/// what made the board feel busy.
function facts(card: CardT): string | null {
  const role = [card.level, card.role].filter(Boolean).join(' ')
  const bits = [role, card.years, card.work_mode, card.employment].filter(Boolean)
  return bits.length ? bits.join(' · ') : null
}

function stack(card: CardT, max = 4): string | null {
  if (card.tags.length === 0) return null
  const shown = card.tags.slice(0, max).join(', ')
  return card.tags.length > max ? `${shown} +${card.tags.length - max}` : shown
}

/// Statuses worth printing: something was done to this posting. Everything
/// else is the resting state and says nothing.
///
/// "undeliverable" is the odd one out — nothing was done, three times over.
/// It is here because a post that cleared the bar and never reached you is the
/// one thing about this system you would want to know immediately.
const ACTIONED = ['sent', 'applied', 'dismissed', 'notified', 'expired', 'undeliverable']

/// The mark on a row that was shown the instant it appeared.
///
/// A dot rather than a word: it is on a minority of rows, it repeats down the
/// list, and the reason is a sentence — which belongs in a tooltip, not in the
/// meta line of a hundred rows.
function InstantDot({ what }: { what: string }) {
  return (
    <span
      aria-label={`on your instant list (${what})`}
      title={`Names something on your instant list — put on the board and sent the moment it was found, whatever it scored (${what}).`}
      className="h-1.5 w-1.5 shrink-0 rounded-full bg-amber-400"
    />
  )
}

/// One row on the radar.
///
/// Two lines, both of them scannable: the title at full contrast, everything
/// else in one muted run. The old version used three lines and two different
/// chip styles, which meant a hundred rows had no shape at all — every piece of
/// text was the same size and colour as every other, so nothing stood out and
/// your eye had nowhere to land.
///
/// The tier is a 2px accent bar rather than a coloured pill: rank is a property
/// of the whole row, and a pill competes with the score for the same job.
export function RadarRow({ card }: { card: CardT }) {
  return (
    <li>
      <a
        href={card.url}
        target="_blank"
        rel="noreferrer noopener"
        className="group flex items-stretch gap-3 rounded-md px-3 py-2.5 transition-colors hover:bg-slate-900/70"
      >
        <span
          aria-hidden
          className={`w-0.5 shrink-0 rounded-full ${TIER_ACCENT[rankOf(card)] ?? TIER_ACCENT.marginal}`}
        />

        <span className="min-w-0 flex-1">
          <span className="block truncate text-sm text-slate-200 group-hover:text-slate-50">
            {card.title}
          </span>
          <Dotted
            className="mt-0.5 block"
            parts={[
              card.company,
              card.location,
              facts(card),
              stack(card),
              // Only statuses that mean something happened. "backfilled" and
              // "scored" are what every untouched row says, so printing them
              // put the same word on the end of a hundred lines.
              ACTIONED.includes(card.status) ? card.status : null,
            ]}
          />
        </span>

        <span className="flex shrink-0 flex-col items-end justify-center gap-1 pl-2">
          <span className="flex items-center gap-1.5">
            {card.instant && <InstantDot what={stack(card, 2) ?? 'watched'} />}
            {card.verdict && <VerdictBadge verdict={card.verdict} title={card.reason ?? undefined} />}
            <Score value={card.score} tier={rankOf(card)} title={card.reason ?? `${card.tier} match`} />
          </span>
          <span className="whitespace-nowrap text-[11px] text-slate-600">{card.age}</span>
        </span>
      </a>
    </li>
  )
}

/// An actionable card: the posting, and the three things you can do about it.
///
/// The draft editor is folded away until you ask for it. It used to be a
/// permanently-open five-row textarea on every card, so ten cards were a wall
/// of identical grey boxes and you couldn't see the jobs for the drafts.
/// Nothing is ever sent without a click either way — a tool that mails
/// strangers on your behalf while you sleep is one bad score from being
/// mortifying.
export function Card({ card, onChanged }: { card: CardT; onChanged: () => void }) {
  const [subject, setSubject] = useState(card.draft_subject ?? '')
  const [body, setBody] = useState(card.draft_body ?? '')
  const [open, setOpen] = useState(false)
  const [why, setWhy] = useState(false)
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

  const byEmail = card.apply_kind === 'email'

  return (
    <article className="overflow-hidden rounded-lg bg-slate-900/40 ring-1 ring-slate-800/80">
      {/* The accent runs the full height of the card rather than sitting inside
          it: rank is a property of the whole posting, and an inner bar reads as
          a stray artifact floating next to the text. */}
      <div className="flex items-stretch">
        <span
          aria-hidden
          className={`w-0.5 shrink-0 ${TIER_ACCENT[rankOf(card)] ?? TIER_ACCENT.marginal}`}
        />
        <div className="min-w-0 flex-1 p-3">
          <a
            href={card.url}
            target="_blank"
            rel="noreferrer noopener"
            className="block truncate text-sm font-medium text-slate-100 hover:text-sky-300"
          >
            {card.title}
          </a>
          <Dotted
            className="mt-0.5 block"
            parts={[card.company, card.location, facts(card), card.source_label, card.age]}
          />
          {stack(card, 8) && (
            <p className="mt-1 truncate font-mono text-[11px] text-slate-600">{stack(card, 8)}</p>
          )}
          {/* The assessment's own sentence. It replaces the old "matched on
              go, kafka, systems", which read as evidence but was true of every
              posting you would ever look at. */}
          {card.reason ? (
            <p className="mt-1.5 text-xs leading-relaxed text-slate-400">{card.reason}</p>
          ) : (
            card.why && (
              <p className="mt-1.5 text-xs text-slate-500">
                Resume match on <span className="text-slate-400">{card.why}</span>
              </p>
            )
          )}
        </div>

        <div className="flex shrink-0 flex-col items-end gap-1.5 p-3 pl-2">
          <Score value={card.score} tier={rankOf(card)} title={card.reason ?? `${card.tier} match`} />
          <span className="flex items-center gap-1.5">
            {card.instant && <InstantDot what={stack(card, 2) ?? 'watched'} />}
            {card.verdict && <VerdictBadge verdict={card.verdict} />}
          </span>
        </div>
      </div>

      {/* Actions, on their own quiet strip. Separated from the content so the
          eye reads the job first and the buttons second, which is the order you
          actually make the decision in. */}
      <div className="flex flex-wrap items-center gap-1 border-t border-slate-800/80 bg-slate-950/40 px-3 py-2">
        <Button onClick={() => setOpen(!open)} tone={open ? 'normal' : 'quiet'}>
          {open ? 'Hide draft' : byEmail ? 'Write email' : 'Draft message'}
        </Button>

        {card.dimensions.length > 0 && (
          <Button onClick={() => setWhy(!why)} tone={why ? 'normal' : 'quiet'}>
            {why ? 'Hide scoring' : 'Why this score'}
          </Button>
        )}

        {!byEmail && card.apply_target && (
          <a
            href={card.apply_target}
            target="_blank"
            rel="noreferrer noopener"
            className="rounded-md px-2.5 py-1.5 text-xs text-sky-300 transition-colors hover:bg-sky-500/10 hover:text-sky-200"
          >
            {card.apply_kind === 'dm' ? 'Open profile ↗' : 'Open listing ↗'}
          </a>
        )}

        <span className="flex-1" />

        {note && (
          <span className={`px-1 text-xs ${note.ok ? 'text-emerald-400' : 'text-rose-400'}`}>
            {note.text}
          </span>
        )}
        <Button disabled={busy} onClick={() => run(() => markApplied(card.id), 'applied')}>
          Applied
        </Button>
        <Button
          disabled={busy}
          tone="danger"
          onClick={() => run(() => dismissCandidate(card.id), 'dismissed')}
        >
          Dismiss
        </Button>
      </div>

      {why && (
        <div className="space-y-3 border-t border-slate-800/80 bg-slate-950/30 p-3">
          <Breakdown dimensions={card.dimensions} />
          {card.missing.length > 0 && (
            <p className="text-xs leading-relaxed text-slate-500">
              <span className="text-slate-600">Missing:</span> {card.missing.join(', ')}
            </p>
          )}
        </div>
      )}

      {open && (
        <div className="space-y-2 border-t border-slate-800/80 p-3">
          {byEmail && (
            <input
              value={subject}
              onChange={(e) => {
                setSubject(e.target.value)
                setDirty(true)
              }}
              placeholder="Subject"
              className={`${inputClass} w-full`}
            />
          )}
          <textarea
            value={body}
            onChange={(e) => {
              setBody(e.target.value)
              setDirty(true)
            }}
            rows={7}
            className={`${inputClass} w-full font-mono leading-relaxed`}
          />
          <div className="flex items-center gap-2">
            {byEmail ? (
              <Button
                tone="primary"
                disabled={busy}
                onClick={() => run(() => sendCandidate(card.id, subject, body), 'sent')}
              >
                Send to {card.apply_target}
              </Button>
            ) : (
              // Nothing to submit to: a DM gets pasted into LinkedIn, an
              // external listing has its own form. So the draft is saved for
              // copying, and you mark it applied once you actually have.
              <Button
                tone="normal"
                disabled={busy}
                onClick={() => run(() => saveDraft(card.id, subject, body), 'saved')}
              >
                Save draft
              </Button>
            )}
            {dirty && <span className="text-xs text-slate-600">unsaved changes</span>}
          </div>
        </div>
      )}
    </article>
  )
}
