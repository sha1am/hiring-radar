import { useCallback, useEffect, useState } from 'react'
import { getBootstrap, getOutbox, getRadar, getStatus } from './api'
import { useEvents, useFilters, useView, type View } from './hooks'
import type { Bootstrap, Card as CardT, OutboxPage, RadarPage, Status } from './types'
import { Card, RadarRow } from './components/Card'
import { FacetChips, FilterBar, TagChips } from './components/Filters'
import { SettingsPanel } from './components/SettingsView'
import { StatusPanel } from './components/StatusPanel'

const LEVEL_DOT: Record<string, string> = {
  ok: 'bg-emerald-400',
  info: 'bg-sky-400',
  warn: 'bg-amber-400',
  error: 'bg-rose-400',
}

/// One tab per thing you do, rather than one page with everything on it.
///
/// The three activities barely overlap: triaging what's waiting, scanning
/// everything crawled, and working through drafts are different sittings, and
/// stacking them meant the list you wanted was always below two you didn't.
/// Status gets its own tab for the same reason — it is what you open when
/// something is wrong, not something to read past every time.
function Nav({
  view,
  go,
  counts,
  level,
}: {
  view: View
  go: (v: View) => void
  counts: Partial<Record<View, number>>
  level: string
}) {
  const TABS: [View, string][] = [
    ['queue', 'queue'],
    ['radar', 'radar'],
    ['outbox', 'outbox'],
    ['status', 'status'],
    ['settings', 'settings'],
  ]

  return (
    <header className="mb-5">
      <div className="mb-3 flex items-center gap-2">
        <span aria-hidden className="text-lg">
          📡
        </span>
        <h1 className="text-base font-semibold text-slate-100">Hiring Radar</h1>
      </div>

      <nav className="flex flex-wrap items-center gap-1 border-b border-slate-800 pb-px">
        {TABS.map(([v, label]) => {
          const n = counts[v]
          const active = view === v
          return (
            <button
              key={v}
              onClick={() => go(v)}
              aria-current={active ? 'page' : undefined}
              className={`-mb-px flex items-center gap-1.5 border-b-2 px-3 py-1.5 text-xs transition-colors ${
                active
                  ? 'border-sky-400 text-slate-100'
                  : 'border-transparent text-slate-400 hover:text-slate-200'
              }`}
            >
              {/* The status tab carries the severity dot, so a failing source is
                  visible from any tab without having to open this one. */}
              {v === 'status' && (
                <span className={`h-1.5 w-1.5 rounded-full ${LEVEL_DOT[level] ?? 'bg-slate-600'}`} />
              )}
              {label}
              {n ? (
                <span className="rounded-full bg-slate-800 px-1.5 py-0.5 text-[10px] text-slate-300">
                  {n}
                </span>
              ) : null}
            </button>
          )
        })}
      </nav>
    </header>
  )
}

function Empty({ children }: { children: React.ReactNode }) {
  return <p className="py-10 text-center text-sm text-slate-500">{children}</p>
}

/// The one-line diagnosis, on every tab except Status.
///
/// The detail belongs on its own tab, but "every target failed" is not
/// something to discover by navigating — an empty queue and a broken crawler
/// look identical, and the whole point is that you shouldn't have to guess.
function Banner({ status, go }: { status: Status; go: (v: View) => void }) {
  if (status.level === 'ok') return null
  const tone =
    status.level === 'error'
      ? 'bg-rose-500/10 text-rose-300 ring-rose-500/30'
      : status.level === 'warn'
        ? 'bg-amber-500/10 text-amber-300 ring-amber-500/30'
        : 'bg-sky-500/10 text-sky-300 ring-sky-500/30'
  return (
    <button
      onClick={() => go('status')}
      className={`mb-4 flex w-full items-start gap-2 rounded-lg px-3 py-2 text-left text-xs ring-1 ${tone}`}
    >
      <span className="flex-1">{status.message}</span>
      <span className="shrink-0 opacity-60">details →</span>
    </button>
  )
}

function QueueView({ queue, reload }: { queue: CardT[]; reload: () => void }) {
  return (
    <section className="space-y-3">
      <h2 className="text-sm uppercase tracking-wide text-slate-400">
        Waiting on you <span className="text-slate-600">· {queue.length}</span>
      </h2>
      {queue.length === 0 ? (
        <Empty>
          Nothing is waiting. Alerts land here when a post clears the strong bar — everything else
          is on the radar, and everything worth applying to is in the outbox.
        </Empty>
      ) : (
        queue.map((c) => <Card key={c.id} card={c} onChanged={reload} />)
      )}
    </section>
  )
}

function RadarView({
  radar,
  filters,
  setFilters,
}: {
  radar: RadarPage
  filters: ReturnType<typeof useFilters>[0]
  setFilters: ReturnType<typeof useFilters>[1]
}) {
  return (
    <section className="space-y-3">
      <h2 className="text-sm uppercase tracking-wide text-slate-400">Everything crawled</h2>
      <FilterBar
        filters={filters}
        setFilters={setFilters}
        sources={radar.sources}
        statuses={radar.statuses}
        shown={radar.items.length}
        total={radar.total_in_window}
      />
      <div className="space-y-1.5">
        <FacetChips
          label="role"
          facets={radar.roles}
          selected={filters.roles}
          onChange={(v) => setFilters({ ...filters, roles: v })}
        />
        <FacetChips
          label="level"
          facets={radar.levels}
          selected={filters.levels}
          onChange={(v) => setFilters({ ...filters, levels: v })}
        />
        <FacetChips
          label="where"
          facets={radar.work_modes}
          selected={filters.modes}
          onChange={(v) => setFilters({ ...filters, modes: v })}
        />
        <TagChips filters={filters} setFilters={setFilters} tags={radar.tags} />
      </div>

      {/* The facts sharpen as the model works through the backlog, so a board
          that quietly changes under you says why. */}
      {radar.pending_enrichment > 0 && (
        <p className="text-xs text-slate-600">
          {radar.pending_enrichment} posting{radar.pending_enrichment === 1 ? '' : 's'} still being
          read — role, level and years will sharpen as that finishes.
        </p>
      )}
      {radar.items.length === 0 ? (
        <Empty>
          Nothing matches those filters in the last {filters.hours}h. Widen the window, or clear the
          filters.
        </Empty>
      ) : (
        <ul className="divide-y divide-slate-900">
          {radar.items.map((c) => (
            <RadarRow key={c.id} card={c} />
          ))}
        </ul>
      )}
    </section>
  )
}

function OutboxView({
  outbox,
  reload,
  go,
}: {
  outbox: OutboxPage
  reload: () => void
  go: (v: View) => void
}) {
  return (
    <section className="space-y-3">
      <div className="flex items-center justify-between">
        <h2 className="text-sm uppercase tracking-wide text-slate-400">
          Outbox{' '}
          <span className="normal-case text-slate-600">
            · {outbox.total} at {outbox.min_score}+
          </span>
        </h2>
        <button onClick={() => go('settings')} className="text-xs text-slate-500 hover:text-slate-300">
          tune the bar →
        </button>
      </div>
      {/* Deliberately not the same list as the queue. That one holds only what
          fired a notification, and notifications are capped at a few an hour —
          a post that scored 88 on a busy morning is no less worth applying to
          for having missed a slot. */}
      {outbox.items.length === 0 ? (
        <Empty>
          Nothing at or above {outbox.min_score} yet. Lower the bar in settings, or wait for the next
          crawl.
        </Empty>
      ) : (
        outbox.items.map((c) => <Card key={c.id} card={c} onChanged={reload} />)
      )}
    </section>
  )
}

export default function App() {
  const [view, go] = useView()
  // 24h until the server says otherwise; the first bootstrap corrects it.
  const [filters, setFilters] = useFilters(24)
  const [data, setData] = useState<Bootstrap | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [stale, setStale] = useState(false)

  /// One request paints the whole dashboard. Four parallel ones would paint it
  /// in four stages, each reflowing the page, and could disagree with each
  /// other about what is in the outbox.
  const loadAll = useCallback(async () => {
    try {
      setData(await getBootstrap(filters))
      setError(null)
      setStale(false)
    } catch (e) {
      // Keep showing the last good data — but say so. Silently rendering stale
      // cards is worse than an empty screen when you are deciding whether to
      // apply to something.
      setError(e instanceof Error ? e.message : 'unreachable')
      setStale(true)
    }
  }, [filters])

  /// A filter change only needs the list, not the status panel and the outbox.
  const loadRadar = useCallback(async () => {
    try {
      const r = await getRadar(filters)
      setData((d) => (d ? { ...d, radar: r } : d))
      setError(null)
      setStale(false)
    } catch (e) {
      setError(e instanceof Error ? e.message : 'unreachable')
      setStale(true)
    }
  }, [filters])

  /// After an action, refresh everything that can have changed — dismissing a
  /// card removes it from the queue, the outbox and the radar at once.
  const reload = useCallback(async () => {
    try {
      const [s, o, r] = await Promise.all([getStatus(), getOutbox(), getRadar(filters)])
      setData((d) => (d ? { ...d, status: s, outbox: o, radar: r } : d))
    } catch {
      // The action itself already reported its own failure.
    }
  }, [filters])

  useEffect(() => {
    if (data) loadRadar()
    else loadAll()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filters])

  useEvents(() => {
    if (view === 'settings') return // don't yank a form out from under an edit
    loadAll()
  })

  if (!data) {
    return (
      <div className="mx-auto max-w-3xl px-4 py-8">
        <Nav view={view} go={go} counts={{}} level="info" />
        {error ? (
          <p className="text-sm text-rose-400">Can't reach the radar — {error}</p>
        ) : (
          <p className="text-sm text-slate-500">Loading…</p>
        )}
      </div>
    )
  }

  return (
    <div className="mx-auto max-w-3xl px-4 py-8">
      <Nav
        view={view}
        go={go}
        level={data.status.level}
        counts={{ queue: data.queue.length, outbox: data.status.outbox_count }}
      />

      {stale && (
        <p className="mb-3 rounded-md bg-rose-500/10 px-3 py-2 text-xs text-rose-300 ring-1 ring-rose-500/30">
          Can't reach the radar ({error}) — showing the last data received.
        </p>
      )}

      {view !== 'status' && view !== 'settings' && <Banner status={data.status} go={go} />}

      {view === 'settings' && <SettingsPanel />}
      {view === 'status' && <StatusPanel status={data.status} />}
      {view === 'outbox' && <OutboxView outbox={data.outbox} reload={reload} go={go} />}
      {view === 'queue' && <QueueView queue={data.queue} reload={reload} />}
      {view === 'radar' && <RadarView radar={data.radar} filters={filters} setFilters={setFilters} />}
    </div>
  )
}
