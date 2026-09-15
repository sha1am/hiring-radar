import { useCallback, useEffect, useRef, useState } from 'react'
import type { Filters, SortKey } from './types'
import { emptyFilters } from './types'

/// Live updates, without a polling loop.
///
/// The server already pushes a tick on `/events` whenever the queue changes, so
/// the client subscribes and refetches rather than asking every few seconds
/// whether anything happened. Polling a mostly-idle radar is almost all wasted
/// requests, and it still leaves a window where a new match sits unseen.
///
/// The ticks are debounced: one crawl can store a dozen posts and fire a dozen
/// events, and refetching twelve times to paint one list is how a dashboard
/// left open all day becomes the noisiest thing on the machine.
export function useEvents(onTick: () => void, debounceMs = 400) {
  const cb = useRef(onTick)
  cb.current = onTick

  useEffect(() => {
    const es = new EventSource('/events')
    let timer: number | undefined
    es.onmessage = () => {
      window.clearTimeout(timer)
      timer = window.setTimeout(() => cb.current(), debounceMs)
    }
    // EventSource reconnects on its own; logging every blip would be noise.
    es.onerror = () => {}
    return () => {
      window.clearTimeout(timer)
      es.close()
    }
  }, [debounceMs])
}

export const VIEWS = ['queue', 'radar', 'outbox', 'status', 'settings'] as const
export type View = (typeof VIEWS)[number]

/// Which tab is showing, kept in the URL hash.
///
/// Not a router dependency: five flat views, no parameters, no nesting. What
/// the hash buys is that Back works and a tab can be bookmarked or sent to
/// yourself — which is the actual reason people notice a missing router, not
/// the routing itself.
export function useView(): [View, (v: View) => void] {
  const read = (): View => {
    const h = window.location.hash.replace(/^#\/?/, '').split('?')[0]
    return (VIEWS as readonly string[]).includes(h) ? (h as View) : 'queue'
  }
  const [view, setView] = useState<View>(read)

  useEffect(() => {
    const onHash = () => setView(read())
    window.addEventListener('hashchange', onHash)
    return () => window.removeEventListener('hashchange', onHash)
  }, [])

  const go = useCallback((v: View) => {
    // The radar's filters live in the same hash, so switching tabs keeps them —
    // coming back to a board you had filtered and finding it reset is the
    // failure this whole scheme exists to avoid.
    const qs = window.location.hash.split('?')[1]
    window.location.hash = `/${v}${qs ? `?${qs}` : ''}`
    setView(v)
  }, [])

  return [view, go]
}

/// Filters, mirrored into the query part of the hash.
///
/// A filtered board is something you send to yourself, or leave open and come
/// back to after a reload. If the filters live only in component state, both of
/// those silently reset to "everything" — and you don't notice, because a board
/// showing everything looks exactly like a board showing what you asked for.
export function useFilters(defaultHours: number): [Filters, (f: Filters) => void] {
  const read = useCallback((): Filters => {
    const qs = window.location.hash.split('?')[1] ?? ''
    const p = new URLSearchParams(qs)
    const hours = Number(p.get('hours'))
    return {
      hours: Number.isFinite(hours) && hours > 0 ? hours : defaultHours,
      q: p.get('q') ?? '',
      source: p.get('source') ?? '',
      status: p.get('status') ?? '',
      tier: p.get('tier') ?? '',
      min: p.get('min') ?? '',
      tags: (p.get('tags') ?? '').split(',').filter(Boolean),
      roles: (p.get('roles') ?? '').split(',').filter(Boolean),
      levels: (p.get('levels') ?? '').split(',').filter(Boolean),
      modes: (p.get('modes') ?? '').split(',').filter(Boolean),
      regions: (p.get('regions') ?? '').split(',').filter(Boolean),
      yrsHave: p.get('yrs_have') ?? '',
      sort: asSort(p.get('sort')),
    }
  }, [defaultHours])

  const [filters, setFiltersState] = useState<Filters>(read)

  const setFilters = useCallback((f: Filters) => {
    setFiltersState(f)
    const p = new URLSearchParams()
    if (f.hours) p.set('hours', String(f.hours))
    if (f.q) p.set('q', f.q)
    if (f.source) p.set('source', f.source)
    if (f.status) p.set('status', f.status)
    if (f.tier) p.set('tier', f.tier)
    if (f.min) p.set('min', f.min)
    if (f.tags.length) p.set('tags', f.tags.join(','))
    if (f.roles.length) p.set('roles', f.roles.join(','))
    if (f.levels.length) p.set('levels', f.levels.join(','))
    if (f.modes.length) p.set('modes', f.modes.join(','))
    if (f.regions.length) p.set('regions', f.regions.join(','))
    if (f.yrsHave) p.set('yrs_have', f.yrsHave)
    if (f.sort !== 'newest') p.set('sort', f.sort)
    const base = window.location.hash.split('?')[0] || '#/'
    const qs = p.toString()
    // replaceState, not assignment: typing in the search box should not push a
    // history entry per keystroke, or Back becomes useless.
    history.replaceState(null, '', qs ? `${base}?${qs}` : base)
  }, [])

  return [filters, setFilters]
}

/// A stale bookmark carrying ?sort=priority should show the board, not break.
const asSort = (v: string | null): SortKey =>
  v === 'oldest' || v === 'score' ? v : 'newest'

export const clearFilters = (hours: number) => emptyFilters(hours)

/// Debounce a value — for the search box, so every keystroke isn't a request.
export function useDebounced<T>(value: T, ms = 250): T {
  const [v, setV] = useState(value)
  useEffect(() => {
    const t = window.setTimeout(() => setV(value), ms)
    return () => window.clearTimeout(t)
  }, [value, ms])
  return v
}
