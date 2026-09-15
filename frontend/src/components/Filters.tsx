import { useEffect, useState } from 'react'
import type { Facet, Filters as F } from '../types'
import { emptyFilters } from '../types'
import { useDebounced } from '../hooks'

const WINDOWS: [number, string][] = [
  [6, '6h'],
  [24, '24h'],
  [72, '3d'],
  [168, '7d'],
]

function Select({
  value,
  onChange,
  options,
  placeholder,
}: {
  value: string
  onChange: (v: string) => void
  options: Facet[]
  placeholder: string
}) {
  return (
    <select
      value={value}
      onChange={(e) => onChange(e.target.value)}
      className="rounded-md border border-slate-700 bg-slate-900/60 px-2 py-1 text-xs text-slate-300"
    >
      <option value="">{placeholder}</option>
      {options.map((o) => (
        <option key={o.value} value={o.value}>
          {o.label}
        </option>
      ))}
    </select>
  )
}

export function FilterBar({
  filters,
  setFilters,
  sources,
  statuses,
  shown,
  total,
}: {
  filters: F
  setFilters: (f: F) => void
  sources: Facet[]
  statuses: Facet[]
  shown: number
  total: number
}) {
  // The search box is local state debounced into the filters, so typing feels
  // instant and doesn't fire a request per keystroke.
  const [q, setQ] = useState(filters.q)
  const debounced = useDebounced(q)
  useEffect(() => {
    if (debounced !== filters.q) setFilters({ ...filters, q: debounced })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [debounced])
  useEffect(() => setQ(filters.q), [filters.q])

  const active =
    filters.q || filters.source || filters.status || filters.tier || filters.min || filters.tags.length

  return (
    <div className="space-y-2">
      <div className="flex flex-wrap items-center gap-2">
        <div className="flex overflow-hidden rounded-md ring-1 ring-slate-700">
          {WINDOWS.map(([h, label]) => (
            <button
              key={h}
              onClick={() => setFilters({ ...filters, hours: h })}
              className={`px-2 py-1 text-xs ${
                filters.hours === h
                  ? 'bg-slate-700 text-slate-100'
                  : 'bg-slate-900/60 text-slate-400 hover:text-slate-200'
              }`}
            >
              {label}
            </button>
          ))}
        </div>

        <input
          value={q}
          onChange={(e) => setQ(e.target.value)}
          placeholder="search title, company, location…"
          className="min-w-[10rem] flex-1 rounded-md border border-slate-700 bg-slate-900/60 px-2 py-1 text-xs text-slate-200 placeholder:text-slate-600"
        />

        <Select
          value={filters.source}
          onChange={(v) => setFilters({ ...filters, source: v })}
          options={sources}
          placeholder="any source"
        />
        <Select
          value={filters.status}
          onChange={(v) => setFilters({ ...filters, status: v })}
          options={statuses}
          placeholder="any status"
        />
        <Select
          value={filters.tier}
          onChange={(v) => setFilters({ ...filters, tier: v })}
          options={[
            { value: 'exceptional', label: 'exceptional', count: null },
            { value: 'strong', label: 'strong', count: null },
            { value: 'marginal', label: 'marginal', count: null },
          ]}
          placeholder="any tier"
        />
        <input
          type="number"
          value={filters.min}
          onChange={(e) => setFilters({ ...filters, min: e.target.value })}
          placeholder="min"
          className="w-16 rounded-md border border-slate-700 bg-slate-900/60 px-2 py-1 text-xs text-slate-200 placeholder:text-slate-600"
        />

        {active ? (
          <button
            onClick={() => setFilters(emptyFilters(filters.hours))}
            className="text-xs text-slate-500 hover:text-slate-300"
          >
            clear
          </button>
        ) : null}
      </div>

      <p className="text-xs text-slate-600">
        {active ? `${shown} of ${total}` : `${total}`} in the last {filters.hours}h
      </p>
    </div>
  )
}

/// Toggleable technology chips.
///
/// The search box already accepts "golang", but typing is the wrong interface
/// for scanning: you want to flip between Go and Go-or-Rust without composing a
/// query. Selected chips OR together — chips are a net for scanning, not a
/// sieve, so picking two widens the board rather than narrowing it to the
/// intersection, which would almost always be empty.
export function TagChips({
  filters,
  setFilters,
  tags,
}: {
  filters: F
  setFilters: (f: F) => void
  tags: Facet[]
}) {
  if (tags.length === 0) return null

  const toggle = (t: string) => {
    const on = filters.tags.includes(t)
    setFilters({
      ...filters,
      tags: on ? filters.tags.filter((x) => x !== t) : [...filters.tags, t],
    })
  }

  return (
    <div className="flex flex-wrap items-center gap-1.5">
      {tags.slice(0, 24).map((t) => {
        const on = filters.tags.includes(t.value)
        return (
          <button
            key={t.value}
            onClick={() => toggle(t.value)}
            aria-pressed={on}
            className={`rounded-full px-2 py-0.5 text-xs ring-1 transition-colors ${
              on
                ? 'bg-sky-500/20 text-sky-200 ring-sky-500/40'
                : 'bg-slate-800/60 text-slate-400 ring-slate-700/60 hover:text-slate-200'
            }`}
          >
            {t.label}
            {t.count !== null && <span className="ml-1 opacity-60">{t.count}</span>}
          </button>
        )
      })}
    </div>
  )
}
