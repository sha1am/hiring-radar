import { useCallback, useEffect, useState } from 'react'
import { getBootstrap, getOutbox, getRadar, getStatus } from './api'
import { useEvents, useFilters, useView } from './hooks'
import type { Bootstrap, Card as CardT, OutboxPage, RadarPage } from './types'
import { Card, RadarRow } from './components/Card'
import { FilterBar, TagChips } from './components/Filters'
import { SettingsPanel } from './components/SettingsView'
import { StatusPanel } from './components/StatusPanel'

function Nav({
  view,
  go,
  outboxCount,
}: {
  view: string
  go: (v: 'radar' | 'outbox' | 'settings') => void
  outboxCount: number
}) {
  const tab = (v: 'radar' | 'outbox' | 'settings', label: string, badge?: number) => (
    <button
      onClick={() => go(v)}
      className={`rounded-md px-2.5 py-1 text-xs transition-colors ${
        view === v ? 'bg-slate-800 text-slate-100' : 'text-slate-400 hover:text-slate-200'
      }`}
    >
      {label}
      {badge ? (
        <span className="ml-1.5 rounded-full bg-sky-500/20 px-1.5 py-0.5 text-sky-300">{badge}</span>
      ) : null}
    </button>
  )

  return (
    <header className="mb-5 flex items-center justify-between">
      <button onClick={() => go('radar')} className="flex items-center gap-2">
        <span aria-hidden className="text-lg">
          📡
        </span>
        <h1 className="text-base font-semibold text-slate-100">Hiring Radar</h1>
      </button>
      <nav className="flex items-center gap-1">
        {tab('radar', 'radar')}
        {tab('outbox', 'outbox', outboxCount)}
        {tab('settings', 'settings')}
      </nav>
    </header>
  )
}

function Empty({ children }: { children: React.ReactNode }) {
  return <p className="py-8 text-center text-sm text-slate-500">{children}</p>
}

function QueueSection({ queue, reload }: { queue: CardT[]; reload: () => void }) {
  if (queue.length === 0) return null
  return (
    <section className="space-y-3">
      <h2 className="text-sm uppercase tracking-wide text-slate-400">
        Waiting on you <span className="text-slate-600">· {queue.length}</span>
      </h2>
      {queue.map((c) => (
        <Card key={c.id} card={c} onChanged={reload} />
      ))}
    </section>
  )
}

function RadarSection({
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
      <TagChips filters={filters} setFilters={setFilters} tags={radar.tags} />
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

function OutboxSection({ outbox, reload }: { outbox: OutboxPage; reload: () => void }) {
  return (
    <section className="space-y-3">
      <div className="flex items-center justify-between">
        <h2 className="text-sm uppercase tracking-wide text-slate-400">
          Outbox{' '}
          <span className="normal-case text-slate-600">
            · {outbox.total} at {outbox.min_score}+
          </span>
        </h2>
        <a href="#/settings" className="text-xs text-slate-500 hover:text-slate-300">
          tune the bar →
        </a>
      </div>
      {/* Deliberately not the same list as the alert queue. That one holds only
          what fired a notification, and notifications are capped at a few an
          hour — a post that scored 88 on a busy morning is no less worth
          applying to for having missed a slot. */}
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
      const b = await getBootstrap(filters)
      setData(b)
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
        <Nav view={view} go={go} outboxCount={0} />
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
      <Nav view={view} go={go} outboxCount={data.status.outbox_count} />

      {stale && (
        <p className="mb-3 rounded-md bg-rose-500/10 px-3 py-2 text-xs text-rose-300 ring-1 ring-rose-500/30">
          Can't reach the radar ({error}) — showing the last data received.
        </p>
      )}

      {view === 'settings' ? (
        <SettingsPanel />
      ) : view === 'outbox' ? (
        <OutboxSection outbox={data.outbox} reload={reload} />
      ) : (
        <div className="space-y-8">
          <StatusPanel status={data.status} />
          <QueueSection queue={data.queue} reload={reload} />
          <RadarSection radar={data.radar} filters={filters} setFilters={setFilters} />
        </div>
      )}
    </div>
  )
}
