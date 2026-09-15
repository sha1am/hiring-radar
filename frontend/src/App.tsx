import { useCallback, useEffect, useState, type ReactNode } from 'react'
import { getBootstrap, getOutbox, getRadar, getStatus } from './api'
import { useEvents, useFilters, useView, type View } from './hooks'
import type { Bootstrap, Card as CardT, OutboxPage, RadarPage, Status } from './types'
import { Card, RadarRow } from './components/Card'
import { FilterBar } from './components/Filters'
import { SettingsPanel } from './components/SettingsView'
import { StatusPanel } from './components/StatusPanel'
import { Empty, Heading, Skeleton } from './ui'

const LEVEL_DOT: Record<string, string> = {
  ok: 'bg-emerald-400',
  info: 'bg-sky-400',
  warn: 'bg-amber-400',
  error: 'bg-rose-400',
}

/// One tab per thing you do.
///
/// Triaging what's waiting, scanning everything crawled, and working through
/// drafts are separate sittings; stacking them on one page meant the list you
/// wanted was always below two you didn't.
///
/// Sticky, because the radar is a long list and losing the tabs the moment you
/// scroll is how a dashboard starts feeling like a document.
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
    ['queue', 'Queue'],
    ['radar', 'Radar'],
    ['outbox', 'Outbox'],
    ['status', 'Status'],
    ['settings', 'Settings'],
  ]

  return (
    <header className="sticky top-0 z-10 mb-6 border-b border-slate-800/80 bg-slate-950/85 backdrop-blur">
      <div className="mx-auto max-w-4xl px-4">
        <div className="flex items-center gap-2 pt-4">
          <span aria-hidden className="text-base">
            📡
          </span>
          <span className="text-sm font-semibold tracking-tight text-slate-100">Hiring Radar</span>
        </div>

        <nav className="flex items-center gap-0.5 overflow-x-auto">
          {TABS.map(([v, label]) => {
            const n = counts[v]
            const active = view === v
            return (
              <button
                key={v}
                onClick={() => go(v)}
                aria-current={active ? 'page' : undefined}
                className={`relative flex items-center gap-1.5 whitespace-nowrap px-3 py-2.5 text-xs transition-colors ${
                  active ? 'text-slate-100' : 'text-slate-500 hover:text-slate-300'
                }`}
              >
                {/* The status tab carries the severity dot, so a failing source
                    is visible from any tab without opening this one. */}
                {v === 'status' && (
                  <span
                    className={`h-1.5 w-1.5 rounded-full ${LEVEL_DOT[level] ?? 'bg-slate-600'}`}
                  />
                )}
                {label}
                {n ? (
                  <span className="rounded-full bg-slate-800 px-1.5 text-[10px] tabular-nums text-slate-300">
                    {n}
                  </span>
                ) : null}
                {active && <span aria-hidden className="absolute inset-x-2 bottom-0 h-px bg-slate-100" />}
              </button>
            )
          })}
        </nav>
      </div>
    </header>
  )
}

/// The one-line diagnosis, on every tab except Status itself.
///
/// An empty queue and a broken crawler look identical, and finding out which
/// one you have should not require navigating.
function Banner({ status, go }: { status: Status; go: (v: View) => void }) {
  if (status.level === 'ok') return null
  const tone =
    status.level === 'error'
      ? 'bg-rose-500/10 text-rose-200 ring-rose-500/20'
      : status.level === 'warn'
        ? 'bg-amber-500/10 text-amber-200 ring-amber-500/20'
        : 'bg-sky-500/10 text-sky-200 ring-sky-500/20'
  return (
    <button
      onClick={() => go('status')}
      className={`mb-5 flex w-full items-center gap-3 rounded-lg px-3 py-2.5 text-left text-xs leading-relaxed ring-1 ${tone}`}
    >
      {/* Clamped: this message lists every failing target, and with four
          sources down it filled a phone screen before the board began. The
          Status tab has all of it, per source, which is where you go to act on
          it anyway. */}
      <span className="line-clamp-2 flex-1">{status.message}</span>
      <span className="shrink-0 whitespace-nowrap opacity-60">Details →</span>
    </button>
  )
}

function QueueView({ queue, reload }: { queue: CardT[]; reload: () => void }) {
  return (
    <section className="space-y-3">
      <Heading>Waiting on you</Heading>
      {queue.length === 0 ? (
        <Empty title="Nothing is waiting.">
          Alerts land here when a posting clears the strong bar. Everything crawled is on the Radar,
          and everything worth applying to is in the Outbox.
        </Empty>
      ) : (
        <div className="space-y-2">
          {queue.map((c) => (
            <Card key={c.id} card={c} onChanged={reload} />
          ))}
        </div>
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
    <section className="space-y-4">
      <FilterBar
        filters={filters}
        setFilters={setFilters}
        sources={radar.sources}
        statuses={radar.statuses}
        regions={radar.regions}
        roles={radar.roles}
        levels={radar.levels}
        workModes={radar.work_modes}
        tags={radar.tags}
        shown={radar.items.length}
        total={radar.total_in_window}
      />

      {radar.items.length === 0 ? (
        <Empty title={`Nothing here in the last ${filters.hours}h.`}>
          Widen the window or clear the filters.
          {/* The region filter sits downstream of scoring, and scoring is where
              a location outside your profile quietly loses ten points and falls
              under the floor. Somewhere you never listed looks like somewhere
              nobody is hiring — worth saying at the moment it bites. */}
          {filters.regions.length > 0 && (
            <>
              {' '}
              Looking outside the places in your profile? Jobs elsewhere lose the location bonus and
              often fall under the score floor — add those places, or set location to “off”, in{' '}
              <a href="#/settings" className="underline hover:text-slate-400">
                settings
              </a>
              .
            </>
          )}
        </Empty>
      ) : (
        <ul className="-mx-3 divide-y divide-slate-900/70">
          {radar.items.map((c) => (
            <RadarRow key={c.id} card={c} />
          ))}
        </ul>
      )}

      {/* The facts sharpen as the model works through the backlog, so a board
          that quietly changes under you says why. */}
      {radar.pending_enrichment > 0 && (
        <p className="text-center text-xs text-slate-600">
          {radar.pending_enrichment} posting{radar.pending_enrichment === 1 ? '' : 's'} still being
          read — role, level and years will sharpen as that finishes.
        </p>
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
      <Heading
        right={
          <button
            onClick={() => go('settings')}
            className="text-xs text-slate-500 hover:text-slate-300"
          >
            Bar at {outbox.min_score} →
          </button>
        }
      >
        Worth applying to
      </Heading>
      {/* Deliberately not the same list as the queue. That one holds only what
          fired a notification, and notifications are capped at a few an hour —
          a posting that scored 88 on a busy morning is no less worth applying
          to for having missed a slot. */}
      {outbox.items.length === 0 ? (
        <Empty title={`Nothing at or above ${outbox.min_score} yet.`}>
          Lower the bar in settings, or wait for the next crawl.
        </Empty>
      ) : (
        <div className="space-y-2">
          {outbox.items.map((c) => (
            <Card key={c.id} card={c} onChanged={reload} />
          ))}
        </div>
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

  const shell = (children: ReactNode, level = 'info', counts: Partial<Record<View, number>> = {}) => (
    <div className="min-h-screen">
      <Nav view={view} go={go} counts={counts} level={level} />
      <main className="mx-auto max-w-4xl px-4 pb-24">{children}</main>
    </div>
  )

  if (!data) {
    return shell(
      error ? (
        <Empty title="Can’t reach the radar.">
          {error}. The page recovers on its own once the server answers.
        </Empty>
      ) : (
        <Skeleton rows={6} />
      ),
    )
  }

  const body =
    view === 'settings' ? (
      <SettingsPanel />
    ) : view === 'status' ? (
      <StatusPanel status={data.status} />
    ) : view === 'outbox' ? (
      <OutboxView outbox={data.outbox} reload={reload} go={go} />
    ) : view === 'queue' ? (
      <QueueView queue={data.queue} reload={reload} />
    ) : (
      <RadarView radar={data.radar} filters={filters} setFilters={setFilters} />
    )

  return shell(
    <>
      {stale && (
        <p className="mb-5 rounded-lg bg-rose-500/10 px-3 py-2.5 text-xs text-rose-200 ring-1 ring-rose-500/20">
          Can’t reach the radar ({error}) — showing the last data received.
        </p>
      )}
      {view !== 'status' && view !== 'settings' && <Banner status={data.status} go={go} />}
      {body}
    </>,
    data.status.level,
    { queue: data.queue.length, outbox: data.status.outbox_count },
  )
}
