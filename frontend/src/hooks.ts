import { useCallback, useEffect, useRef, useState } from 'react'
import type { Filters } from './types'
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

export type View = 'radar' | 'outbox' | 'settings'

/// Which view is showing, kept in the URL hash.
///
/// Not a router dependency: three views, no parameters, no nesting. What the
/// hash buys is that the browser back button works and a view can be
/// bookmarked — which is the actual reason people notice a missing router, not
/// the routing itself.
export function useView(): [View, (v: View) => void] {
  const read = (): View => {
    const h = window.location.hash.replace(/^#\/?/, '')
    return h === 'outbox' || h === 'settings' ? h : 'radar'
  }
  const [view, setView] = useState<View>(read)

  useEffect(() => {
    const onHash = () => setView(read())
    window.addEventListener('hashchange', onHash)
    return () => window.removeEventListener('hashchange', onHash)
  }, [])

  const go = useCallback((v: View) => {
    window.location.hash = v === 'radar' ? '/' : `/${v}`
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
    const base = window.location.hash.split('?')[0] || '#/'
    const qs = p.toString()
    // replaceState, not assignment: typing in the search box should not push a
    // history entry per keystroke, or Back becomes useless.
    history.replaceState(null, '', qs ? `${base}?${qs}` : base)
  }, [])

  return [filters, setFilters]
}

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
