/// Shared visual vocabulary.
///
/// Defined once, in one file, because the dashboard's real problem was never
/// any single screen — it was that every component invented its own type scale
/// and its own greys, so nothing lined up and everything looked equally
/// important. Three text sizes, three weights, one accent per meaning.

import type { ReactNode } from 'react'
import type { Dimension } from './types'

/// Rank colour.
///
/// Amber for the best rather than rose: rose reads as an error, and this is the
/// opposite of an error. That leaves rose meaning exactly one thing anywhere in
/// the app — something is wrong.
export const TIER_ACCENT: Record<string, string> = {
  // verdicts, which are the better signal when the ATS has an opinion
  apply: 'bg-amber-400',
  stretch: 'bg-sky-400',
  reach: 'bg-slate-700',
  skip: 'bg-slate-800',
  // tiers, for rows scored before a resume existed
  exceptional: 'bg-amber-400',
  strong: 'bg-sky-400',
  marginal: 'bg-slate-700',
}

/// Which of the two rankings to colour by.
///
/// There are two: the tier, which is a threshold on the score and drives the
/// notification budget, and the verdict, which is the ATS's advice. They can
/// disagree — a posting can score 91 and still be a stretch because you are two
/// years short — and showing an amber bar next to a blue "STRETCH" badge just
/// looks broken. The verdict is the better signal, so it wins where it exists.
export const rankOf = (c: { verdict: string | null; tier: string }): string =>
  c.verdict ?? c.tier

export const TIER_TEXT: Record<string, string> = {
  apply: 'text-amber-300',
  stretch: 'text-sky-300',
  reach: 'text-slate-500',
  skip: 'text-slate-600',
  exceptional: 'text-amber-300',
  strong: 'text-sky-300',
  marginal: 'text-slate-500',
}

/// The verdict: the assessment's own advice, as a word rather than a number.
///
/// A score of 66 tells you nothing on its own — 66 out of what, against whom.
/// "Stretch" is the sentence the number was standing in for, and it is what you
/// act on. Colours follow the tier palette so a board reads consistently: amber
/// is the standout, sky is solid, slate is neither.
export const VERDICT_STYLE: Record<string, string> = {
  apply: 'bg-amber-400/15 text-amber-300 ring-amber-400/30',
  stretch: 'bg-sky-400/15 text-sky-300 ring-sky-400/30',
  reach: 'bg-slate-700/40 text-slate-400 ring-slate-600/40',
  skip: 'bg-slate-800/40 text-slate-600 ring-slate-700/40',
}

export function VerdictBadge({ verdict, title }: { verdict: string; title?: string }) {
  return (
    <span
      title={title}
      className={`shrink-0 rounded px-1.5 py-0.5 text-[10px] uppercase tracking-wide ring-1 ${
        VERDICT_STYLE[verdict] ?? VERDICT_STYLE.reach
      }`}
    >
      {verdict}
    </span>
  )
}

/// The score, shown as its parts.
///
/// A number asks to be trusted. This shows the working: five dimensions, how
/// each one scored, and what each concluded in words. It is the difference
/// between "66" and "66, because the stack matches and you are two years
/// short" — and the second one you can argue with, which is the point.
export function Breakdown({ dimensions }: { dimensions: Dimension[] }) {
  if (dimensions.length === 0) return null
  return (
    <dl className="space-y-1.5">
      {dimensions.map((d) => (
        <div key={d.name} className="grid grid-cols-[4.5rem_3rem_1fr] items-center gap-2">
          <dt className="text-[10px] uppercase tracking-[0.08em] text-slate-600">{d.name}</dt>
          <dd aria-hidden className="h-1 overflow-hidden rounded-full bg-slate-800">
            {/* Weight is width, fit is fill: a dimension worth 35 points reads
                as wider than one worth 10, so the picture matches the maths. */}
            <div
              className={`h-full rounded-full ${
                d.fit >= 0.8 ? 'bg-emerald-400/70' : d.fit >= 0.5 ? 'bg-sky-400/70' : 'bg-slate-600'
              }`}
              style={{ width: `${Math.round(d.fit * 100)}%` }}
            />
          </dd>
          <dd className="truncate text-[11px] text-slate-500" title={d.note}>
            {d.note}
            <span className="ml-1.5 font-mono text-slate-700">
              {(d.fit * d.weight).toFixed(0)}/{d.weight.toFixed(0)}
            </span>
          </dd>
        </div>
      ))}
    </dl>
  )
}

/// A section heading. One style, everywhere.
export function Heading({ children, right }: { children: ReactNode; right?: ReactNode }) {
  return (
    <div className="flex items-baseline justify-between gap-3">
      <h2 className="text-[11px] font-medium uppercase tracking-[0.08em] text-slate-500">
        {children}
      </h2>
      {right}
    </div>
  )
}

/// Secondary text. The single muted style — before this there were four.
export function Meta({ children, className = '' }: { children: ReactNode; className?: string }) {
  return <span className={`text-xs text-slate-500 ${className}`}>{children}</span>
}

/// Interpunct-joined metadata. Skips blanks, so a card missing a location
/// doesn't render a stray separator — which is most of what made these lines
/// look untidy.
///
/// Wraps to a second line rather than truncating. On a phone the single line
/// cut off at "senior backend …", which threw away the stack — the part you
/// most want to see after the title.
export function Dotted({ parts, className = '' }: { parts: (string | null | undefined)[]; className?: string }) {
  const kept = parts.filter((p): p is string => !!p && p.trim() !== '')
  if (kept.length === 0) return null
  return (
    <span className={`line-clamp-2 text-xs leading-relaxed text-slate-500 ${className}`}>
      {kept.map((p, i) => (
        <span key={`${p}-${i}`}>
          {i > 0 && <span className="px-1.5 text-slate-700">·</span>}
          {p}
        </span>
      ))}
    </span>
  )
}

/// The match score. Tabular figures so a column of them doesn't jitter as the
/// numbers change under an SSE update.
export function Score({ value, tier, title }: { value: number; tier: string; title?: string }) {
  return (
    <span
      title={title}
      className={`shrink-0 font-mono text-sm tabular-nums ${TIER_TEXT[tier] ?? TIER_TEXT.marginal}`}
    >
      {value.toFixed(0)}
    </span>
  )
}

export function Button({
  children,
  onClick,
  disabled,
  tone = 'quiet',
  title,
  type = 'button',
}: {
  children: ReactNode
  onClick?: () => void
  disabled?: boolean
  tone?: 'primary' | 'normal' | 'quiet' | 'danger'
  title?: string
  type?: 'button' | 'submit'
}) {
  const tones = {
    primary: 'bg-emerald-500/90 text-slate-900 hover:bg-emerald-400 font-medium',
    normal: 'bg-slate-800 text-slate-100 hover:bg-slate-700 ring-1 ring-slate-700',
    quiet: 'text-slate-400 hover:text-slate-100 hover:bg-slate-800/60',
    danger: 'text-slate-500 hover:text-rose-300 hover:bg-rose-500/10',
  }
  return (
    <button
      type={type}
      onClick={onClick}
      disabled={disabled}
      title={title}
      className={`rounded-md px-2.5 py-1.5 text-xs transition-colors disabled:opacity-40 ${tones[tone]}`}
    >
      {children}
    </button>
  )
}

/// A segmented control — the window picker and the sort picker are the same
/// shape, so they are the same component.
export function Segmented<T extends string | number>({
  options,
  value,
  onChange,
}: {
  options: [T, string, string?][]
  value: T
  onChange: (v: T) => void
}) {
  return (
    <div className="flex shrink-0 overflow-hidden rounded-md bg-slate-900/70 p-0.5 ring-1 ring-slate-800">
      {options.map(([v, label, title]) => (
        <button
          key={String(v)}
          onClick={() => onChange(v)}
          title={title}
          aria-pressed={value === v}
          className={`rounded px-2 py-1 text-xs transition-colors ${
            value === v
              ? 'bg-slate-700/80 text-slate-100'
              : 'text-slate-500 hover:text-slate-200'
          }`}
        >
          {label}
        </button>
      ))}
    </div>
  )
}

export const inputClass =
  'rounded-md border border-slate-800 bg-slate-900/70 px-2.5 py-1.5 text-xs text-slate-200 ' +
  'placeholder:text-slate-600 focus:border-slate-600 focus:outline-none'

/// A placeholder that says what would be here and what to do about it.
export function Empty({ title, children }: { title: string; children?: ReactNode }) {
  return (
    <div className="rounded-lg border border-dashed border-slate-800 px-6 py-12 text-center">
      <p className="text-sm text-slate-400">{title}</p>
      {children && <p className="mx-auto mt-2 max-w-md text-xs leading-relaxed text-slate-600">{children}</p>}
    </div>
  )
}

/// Loading placeholders shaped like the rows they replace, so the page doesn't
/// jump when the data lands.
export function Skeleton({ rows = 5 }: { rows?: number }) {
  return (
    <div className="space-y-px" aria-hidden>
      {Array.from({ length: rows }).map((_, i) => (
        <div key={i} className="flex items-center gap-3 px-3 py-3">
          <div className="h-8 w-0.5 rounded bg-slate-800/80" />
          <div className="flex-1 space-y-2">
            <div
              className="h-3 rounded bg-slate-800/80"
              style={{ width: `${55 + ((i * 13) % 30)}%` }}
            />
            <div
              className="h-2.5 rounded bg-slate-900"
              style={{ width: `${30 + ((i * 17) % 25)}%` }}
            />
          </div>
        </div>
      ))}
    </div>
  )
}
