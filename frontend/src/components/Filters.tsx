import { useEffect, useState } from 'react'
import type { Facet, Filters as F, SortKey } from '../types'
import { emptyFilters } from '../types'
import { useDebounced } from '../hooks'
import { Segmented, inputClass } from '../ui'

/// Two questions that want opposite orderings. "What just landed" is a feed you
/// read newest-first and stop when you recognise something; "what is worth my
/// afternoon" is a ranking, where the best match from six hours ago beats a
/// fresh mediocre one.
const SORTS: [SortKey, string, string][] = [
  ['newest', 'new', 'Most recently posted first'],
  ['score', 'best', 'Highest match score first'],
  ['oldest', 'old', 'Oldest first — for working back through a backlog'],
]

const WINDOWS: [number, string, string][] = [
  [6, '6h', 'Posted in the last 6 hours'],
  [24, '24h', 'Posted in the last day'],
  [72, '3d', 'Posted in the last 3 days'],
  [168, '7d', 'Posted in the last week'],
]

/// The stored keys are terse; these are what a person reads.
/// The verdict keys are terse; these say what they mean.
const VERDICT_LABEL: Record<string, string> = {
  apply: 'Apply',
  stretch: 'Stretch',
  reach: 'Reach',
  skip: 'Skip',
}

const REGION_LABEL: Record<string, string> = {
  india: 'India',
  gulf: 'Gulf',
  sea: 'SE Asia',
  apac: 'APAC',
  europe: 'Europe',
  americas: 'Americas',
  other: 'Elsewhere',
}

/// How many separate things the board is currently narrowed by.
///
/// This number is what makes folding the panel safe. A hidden filter you have
/// forgotten about is how you end up staring at an almost-empty board and
/// concluding nobody is hiring.
export function activeCount(f: F): number {
  return (
    (f.q ? 1 : 0) +
    (f.source ? 1 : 0) +
    (f.status ? 1 : 0) +
    (f.tier ? 1 : 0) +
    (f.min ? 1 : 0) +
    (f.yrsHave ? 1 : 0) +
    f.regions.length +
    f.verdicts.length +
    f.roles.length +
    f.levels.length +
    f.modes.length +
    f.tags.length
  )
}

function ChipRow({
  label,
  facets,
  selected,
  onChange,
  labelFor,
  max = 14,
}: {
  label: string
  facets: Facet[]
  selected: string[]
  onChange: (v: string[]) => void
  labelFor?: Record<string, string>
  max?: number
}) {
  if (facets.length === 0) return null
  const toggle = (v: string) =>
    onChange(selected.includes(v) ? selected.filter((x) => x !== v) : [...selected, v])

  return (
    <div className="grid grid-cols-[3.5rem_1fr] items-start gap-2">
      <span className="pt-1 text-right text-[10px] uppercase tracking-[0.08em] text-slate-600">
        {label}
      </span>
      <div className="flex flex-wrap gap-1">
        {facets.slice(0, max).map((f) => {
          const on = selected.includes(f.value)
          return (
            <button
              key={f.value}
              onClick={() => toggle(f.value)}
              aria-pressed={on}
              className={`rounded-full px-2 py-0.5 text-xs transition-colors ${
                on
                  ? 'bg-slate-200 text-slate-900'
                  : 'bg-slate-800/70 text-slate-400 hover:bg-slate-800 hover:text-slate-200'
              }`}
            >
              {labelFor?.[f.value] ?? f.label}
              {f.count !== null && (
                <span className={`ml-1 ${on ? 'text-slate-500' : 'text-slate-600'}`}>{f.count}</span>
              )}
            </button>
          )
        })}
      </div>
    </div>
  )
}

/// The filter bar.
///
/// One line stays visible — window, search, ordering — and everything else
/// folds behind a toggle. There are eleven ways to narrow this board, and
/// having all eleven on screen meant the list you came to read started halfway
/// down the page.
///
/// Selections within a group OR together; groups AND with each other. "Backend
/// or SRE, in India or the Gulf" is the question people actually ask, and the
/// intersection of two roles is empty by definition.
export function FilterBar({
  filters,
  setFilters,
  sources,
  statuses,
  regions,
  verdicts,
  roles,
  levels,
  workModes,
  tags,
  shown,
  total,
  onDismissAll,
}: {
  filters: F
  setFilters: (f: F) => void
  sources: Facet[]
  statuses: Facet[]
  regions: Facet[]
  verdicts: Facet[]
  roles: Facet[]
  levels: Facet[]
  workModes: Facet[]
  tags: Facet[]
  shown: number
  total: number
  /// Dismiss everything currently listed. Returns how many rows went.
  onDismissAll: () => Promise<number>
}) {
  const [q, setQ] = useState(filters.q)
  const debounced = useDebounced(q)
  useEffect(() => {
    if (debounced !== filters.q) setFilters({ ...filters, q: debounced })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [debounced])
  useEffect(() => setQ(filters.q), [filters.q])

  // Two-step rather than a confirm dialog: a browser confirm is a modal you
  // dismiss without reading, and this is reversible-ish but tedious to undo.
  // The second click states the number, which is the thing worth checking.
  const [arming, setArming] = useState(false)
  const [clearing, setClearing] = useState(false)
  useEffect(() => setArming(false), [filters])

  const n = activeCount(filters)
  // Open when something is already narrowing the board — usually a reload of a
  // filtered URL, where a collapsed panel would hide why the list is short.
  const [open, setOpen] = useState(n > 0)

  const set = (patch: Partial<F>) => setFilters({ ...filters, ...patch })

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center gap-2">
        <Segmented options={WINDOWS} value={filters.hours} onChange={(hours) => set({ hours })} />

        <input
          value={q}
          onChange={(e) => setQ(e.target.value)}
          placeholder="Search titles, companies, places…"
          className={`${inputClass} min-w-[12rem] flex-1`}
        />

        <Segmented options={SORTS} value={filters.sort} onChange={(sort) => set({ sort })} />

        <button
          onClick={() => setOpen(!open)}
          aria-expanded={open}
          className={`flex shrink-0 items-center gap-1.5 rounded-md px-2.5 py-1.5 text-xs transition-colors ${
            n > 0
              ? 'bg-slate-200 text-slate-900'
              : 'text-slate-400 ring-1 ring-slate-800 hover:text-slate-100'
          }`}
        >
          Filters
          {n > 0 && (
            <span className="rounded-full bg-slate-900/20 px-1.5 text-[10px]">{n}</span>
          )}
          <span aria-hidden className="text-[9px] opacity-60">
            {open ? '▲' : '▼'}
          </span>
        </button>
      </div>

      {open && (
        <div className="space-y-2.5 rounded-lg bg-slate-900/40 p-3 ring-1 ring-slate-800/80">
          {/* The verdict first: it is the assessment's own summary, and the
              one cut that answers "what should I do today" rather than "what
              is out there". */}
          <ChipRow
            label="verdict"
            facets={verdicts}
            selected={filters.verdicts}
            onChange={(verdicts) => set({ verdicts })}
            labelFor={VERDICT_LABEL}
          />
          {/* Region next: the coarsest cut of what is out there, and the one
              you make before you care what the role is. */}
          <ChipRow
            label="where"
            facets={regions}
            selected={filters.regions}
            onChange={(regions) => set({ regions })}
            labelFor={REGION_LABEL}
          />
          <ChipRow
            label="role"
            facets={roles}
            selected={filters.roles}
            onChange={(roles) => set({ roles })}
          />
          <ChipRow
            label="level"
            facets={levels}
            selected={filters.levels}
            onChange={(levels) => set({ levels })}
          />
          <ChipRow
            label="setup"
            facets={workModes}
            selected={filters.modes}
            onChange={(modes) => set({ modes })}
          />
          <ChipRow
            label="stack"
            facets={tags}
            selected={filters.tags}
            onChange={(tags) => set({ tags })}
            max={20}
          />

          <div className="grid grid-cols-[3.5rem_1fr] items-start gap-2 border-t border-slate-800/60 pt-2.5">
            <span className="pt-1.5 text-right text-[10px] uppercase tracking-[0.08em] text-slate-600">
              more
            </span>
            <div className="flex flex-wrap items-center gap-2">
              <select
                value={filters.source}
                onChange={(e) => set({ source: e.target.value })}
                className={inputClass}
              >
                <option value="">any source</option>
                {sources.map((o) => (
                  <option key={o.value} value={o.value}>
                    {o.label}
                  </option>
                ))}
              </select>
              <select
                value={filters.status}
                onChange={(e) => set({ status: e.target.value })}
                className={inputClass}
              >
                <option value="">any status</option>
                {statuses.map((o) => (
                  <option key={o.value} value={o.value}>
                    {o.label}
                  </option>
                ))}
              </select>
              <select
                value={filters.tier}
                onChange={(e) => set({ tier: e.target.value })}
                className={inputClass}
              >
                <option value="">any tier</option>
                <option value="exceptional">exceptional</option>
                <option value="strong">strong</option>
                <option value="marginal">marginal</option>
              </select>
              <label className="flex items-center gap-1.5 text-xs text-slate-500">
                score ≥
                <input
                  type="number"
                  value={filters.min}
                  onChange={(e) => set({ min: e.target.value })}
                  className={`${inputClass} w-16`}
                />
              </label>
              {/* Phrased as what you have, not what the job wants — you know
                  your own number, and a listing stating no years passes either
                  way rather than being filtered out for how it was written. */}
              <label
                className="flex items-center gap-1.5 text-xs text-slate-500"
                title="Hides roles asking for more experience than you have"
              >
                I have
                <input
                  type="number"
                  value={filters.yrsHave}
                  onChange={(e) => set({ yrsHave: e.target.value })}
                  className={`${inputClass} w-14`}
                />
                yrs
              </label>
            </div>
          </div>
        </div>
      )}

      <div className="flex items-center justify-between">
        <p className="text-xs text-slate-600">
          <span className="text-slate-400">{n > 0 ? shown : total}</span>
          {n > 0 && ` of ${total}`} in the last {filters.hours}h
        </p>
        <div className="flex items-center gap-3">
          {n > 0 && (
            <button
              // Clears what you're looking for. Keeps the window and the ordering,
              // which are how you're reading the board, not what you're after.
              onClick={() => setFilters({ ...emptyFilters(filters.hours), sort: filters.sort })}
              className="text-xs text-slate-500 hover:text-slate-200"
            >
              Clear filters
            </button>
          )}
          {shown > 0 &&
            (arming ? (
              <span className="flex items-center gap-2 text-xs">
                <span className="text-slate-400">Dismiss {shown}?</span>
                <button
                  disabled={clearing}
                  onClick={async () => {
                    setClearing(true)
                    try {
                      await onDismissAll()
                    } finally {
                      setClearing(false)
                      setArming(false)
                    }
                  }}
                  className="rounded-md bg-rose-500/15 px-2 py-0.5 text-rose-300 hover:bg-rose-500/25 disabled:opacity-50"
                >
                  {clearing ? 'Dismissing…' : 'Yes, dismiss'}
                </button>
                <button
                  onClick={() => setArming(false)}
                  className="text-slate-500 hover:text-slate-300"
                >
                  cancel
                </button>
              </span>
            ) : (
              <button
                onClick={() => setArming(true)}
                title="Dismiss every row this filter is showing. Anything you have sent or applied to is left alone."
                className="text-xs text-slate-500 hover:text-rose-300"
              >
                Dismiss all
              </button>
            ))}
        </div>
      </div>
    </div>
  )
}
