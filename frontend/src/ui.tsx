/// Shared visual vocabulary.
///
/// Defined once, in one file, because the dashboard's real problem was never
/// any single screen — it was that every component invented its own type scale
/// and its own greys, so nothing lined up and everything looked equally
/// important. Three text sizes, three weights, one accent per meaning.

import type { ReactNode } from 'react'

/// Tier colour.
///
/// Amber for the best match rather than rose: rose reads as an error, and this
/// is the opposite of an error. Sky for strong, slate for marginal. That leaves
/// rose free to mean only one thing anywhere in the app — something is wrong.
export const TIER_ACCENT: Record<string, string> = {
  exceptional: 'bg-amber-400',
  strong: 'bg-sky-400',
  marginal: 'bg-slate-700',
}

export const TIER_TEXT: Record<string, string> = {
  exceptional: 'text-amber-300',
  strong: 'text-sky-300',
  marginal: 'text-slate-500',
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
