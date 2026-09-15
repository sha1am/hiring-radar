// Mirrors the shapes in src/api.rs. Kept hand-written rather than generated:
// the API is small, and a hand-written type is the one place a field rename
// shows up as a compile error instead of as an undefined at runtime.

export type Level = 'ok' | 'info' | 'warn' | 'error'

export interface Card {
  id: number
  title: string
  company: string
  location: string | null
  url: string
  source: string
  source_label: string
  score: number
  priority: number
  tier: string
  status: string
  posted_at: number | null
  detected_at: number
  age: string
  tags: string[]
  why: string | null
  apply_kind: string
  apply_target: string | null
  draft_subject: string | null
  draft_body: string | null
  excerpt: string

  // structured facts (see src/enrich.rs)
  role: string | null
  level: string | null
  /** "5+ yrs" / "3–5 yrs", formatted server-side. */
  years: string | null
  work_mode: string | null
  employment: string | null
  /** False while the model still has this row queued. */
  enriched: boolean
}

export interface Facet {
  value: string
  label: string
  /** null means "present, not counted" — render the chip without a badge. */
  count: number | null
}

export interface RadarPage {
  items: Card[]
  window_hours: number
  total_in_window: number
  sources: Facet[]
  statuses: Facet[]
  tags: Facet[]
  roles: Facet[]
  levels: Facet[]
  work_modes: Facet[]
  pending_enrichment: number
  /** What the server actually sorted by — render the control from this. */
  sort: SortKey
}

export type SortKey = 'newest' | 'oldest' | 'score'

export interface Note {
  text: string
  ok: boolean
}

export interface Source {
  name: string
  label: string
  enabled: boolean
  running: boolean
  last_run: number | null
  next_run: number | null
  last_error: string | null
  notes: Note[]
  fetched: number
  new_posts: number
  stored: number
  below_floor: number
  wrong_location: number
  wrong_stack: number
  not_hiring: number
  total_fetched: number
  total_stored: number
  best_score: number
}

export interface Status {
  level: Level
  message: string
  sources: Source[]
  rows_in_window: number
  window_hours: number
  outbox_count: number
}

export interface OutboxPage {
  items: Card[]
  total: number
  min_score: number
}

export interface Bootstrap {
  status: Status
  queue: Card[]
  radar: RadarPage
  outbox: OutboxPage
}

export interface CompanyList {
  kind: string
  path: string
  entries: string[]
  read_only: boolean
  note: string
}

/** The settings row, plus the read-only extras the server sends alongside. */
export interface Settings {
  titles: string[]
  keywords: string[]
  locations: string[]
  remote_ok: boolean
  location_policy: 'off' | 'prefer' | 'require'
  allow_unknown_location: boolean
  seniority: string[]
  stack: string[]
  stack_policy: 'off' | 'prefer' | 'require'
  dealbreakers: string[]
  min_salary: number | null

  resume: string
  resume_filename: string | null
  resume_updated_at: number | null
  resume_weight: number

  per_hour_cap: number
  per_poster_cap: number
  score_floor: number
  strong_min: number
  exceptional_min: number
  settle_strong_secs: number
  candidate_ttl_secs: number
  adaptive_threshold: boolean
  adaptive_start: number
  adaptive_end: number

  mode: string
  greenhouse_enabled: boolean
  greenhouse_boards: string[]
  lever_enabled: boolean
  lever_boards: string[]
  workday_enabled: boolean
  workday_sites: string[]
  workday_lookback_days: number
  linkedin_guest_enabled: boolean
  linkedin_queries: { keywords: string; location: string }[]
  voyager_enabled: boolean
  voyager_queries: string[]
  voyager_query_id: string
  lookback_hours: number
  voyager_max_pages: number
  voyager_poll_pages: number

  radar_hours: number
  outbox_min: number

  // server-added, not writable
  company_lists: CompanyList[]
  has_resume: boolean
  resume_chars: number
}

/** What the radar list is filtered by. Mirrored into the URL hash. */
export interface Filters {
  hours: number
  q: string
  source: string
  status: string
  tier: string
  min: string
  tags: string[]
  roles: string[]
  levels: string[]
  modes: string[]
  /** "I have N years" — show anything asking for at most N. */
  yrsHave: string
  sort: SortKey
}

export const emptyFilters = (hours: number): Filters => ({
  hours,
  q: '',
  source: '',
  status: '',
  tier: '',
  min: '',
  tags: [],
  roles: [],
  levels: [],
  modes: [],
  yrsHave: '',
  // A feed by default: "what just landed" is the question you open this with.
  sort: 'newest',
})
