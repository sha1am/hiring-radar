import type { Bootstrap, Filters, OutboxPage, RadarPage, Settings, Status } from './types'

/// Every call goes through here so failures have one shape.
///
/// A dashboard that silently renders stale data when the server is down is
/// worse than one that says "can't reach the radar" — you are using it to
/// decide whether to apply to something, and "nothing new" and "I can't tell
/// you" are very different answers.
async function req<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, {
    ...init,
    headers: { 'content-type': 'application/json', ...(init?.headers ?? {}) },
  })
  if (!res.ok) {
    let detail = `HTTP ${res.status}`
    try {
      const body = await res.json()
      if (body?.error) detail = body.error
    } catch {
      // A non-JSON error body is still an error; the status code is enough.
    }
    throw new Error(detail)
  }
  return res.json() as Promise<T>
}

/** The filters, as the query string the API expects. */
export function toQuery(f: Filters): string {
  const p = new URLSearchParams()
  p.set('hours', String(f.hours))
  if (f.q) p.set('q', f.q)
  if (f.source) p.set('source', f.source)
  if (f.status) p.set('status', f.status)
  if (f.tier) p.set('tier', f.tier)
  if (f.min) p.set('min', f.min)
  if (f.tags.length) p.set('tags', f.tags.join(','))
  if (f.roles.length) p.set('roles', f.roles.join(','))
  if (f.levels.length) p.set('levels', f.levels.join(','))
  if (f.modes.length) p.set('modes', f.modes.join(','))
  if (f.yrsHave) p.set('yrs_have', f.yrsHave)
  if (f.sort !== 'newest') p.set('sort', f.sort)
  return p.toString()
}

export const getBootstrap = (f: Filters) => req<Bootstrap>(`/api/bootstrap?${toQuery(f)}`)
export const getRadar = (f: Filters) => req<RadarPage>(`/api/radar?${toQuery(f)}`)
export const getStatus = () => req<Status>('/api/status')
export const getOutbox = () => req<OutboxPage>('/api/outbox')
export const getSettings = () => req<Settings>('/api/settings')

/// The server clamps on save and returns what it stored, so the caller must use
/// the response rather than the object it sent — otherwise the form shows a
/// floor of 90 that the engine is treating as something else entirely.
export const putSettings = (s: Settings) =>
  req<Settings>('/api/settings', { method: 'PUT', body: JSON.stringify(s) })

export const sendCandidate = (id: number, subject: string, body: string) =>
  req<{ sent: boolean; status: string }>(`/api/candidate/${id}/send`, {
    method: 'POST',
    body: JSON.stringify({ subject, body }),
  })

export const saveDraft = (id: number, subject: string, body: string) =>
  req<{ saved: boolean }>(`/api/candidate/${id}/draft`, {
    method: 'POST',
    body: JSON.stringify({ subject, body }),
  })

export const dismissCandidate = (id: number) =>
  req<{ status: string }>(`/api/candidate/${id}/dismiss`, { method: 'POST' })

export const markApplied = (id: number) =>
  req<{ status: string }>(`/api/candidate/${id}/applied`, { method: 'POST' })
