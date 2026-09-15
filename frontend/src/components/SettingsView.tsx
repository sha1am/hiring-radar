import { useEffect, useRef, useState } from 'react'
import type { CompanyList, Settings } from '../types'
import { getSettings, putSettings } from '../api'

function Field({
  label,
  hint,
  children,
}: {
  label: string
  hint?: string
  children: React.ReactNode
}) {
  return (
    <label className="block">
      <span className="block text-sm text-slate-300">{label}</span>
      {hint && <span className="mb-1 block text-xs text-slate-500">{hint}</span>}
      {children}
    </label>
  )
}

const input =
  'w-full rounded-md border border-slate-700 bg-slate-900/60 px-3 py-1.5 text-sm text-slate-200'

function ListField({
  label,
  hint,
  value,
  rows = 4,
  onChange,
}: {
  label: string
  hint: string
  value: string[]
  rows?: number
  onChange: (v: string[]) => void
}) {
  // Edited as text, stored as a list. Keeping the raw text in local state means
  // a blank line while you type doesn't vanish under you on every keystroke.
  const [text, setText] = useState(value.join('\n'))
  useEffect(() => setText(value.join('\n')), [value])
  return (
    <Field label={label} hint={hint}>
      <textarea
        rows={rows}
        value={text}
        onChange={(e) => {
          setText(e.target.value)
          onChange(
            e.target.value
              .split('\n')
              .map((s) => s.trim())
              .filter(Boolean),
          )
        }}
        className={`${input} font-mono`}
      />
    </Field>
  )
}

/// A company list: shown, explained, and not editable here.
///
/// The file on disk is the authority and is re-read on every crawl, so an
/// editable copy would be a lie — you would type into it, save, and the next
/// crawl would read the file and ignore you.
function CompanyListView({ list }: { list: CompanyList }) {
  return (
    <Field
      label={`${list.kind} companies`}
      hint={`${list.note} — edit the file, not this box.`}
    >
      <textarea
        rows={Math.min(Math.max(list.entries.length, 3), 10)}
        value={list.entries.join('\n') || `# nothing yet — add one per line to ${list.path}`}
        disabled
        readOnly
        className="w-full cursor-not-allowed rounded-md border border-slate-800 bg-slate-900/30 px-3 py-1.5 font-mono text-sm text-slate-400"
      />
    </Field>
  )
}

function Num({
  label,
  value,
  onChange,
  step = 1,
}: {
  label: string
  value: number
  onChange: (v: number) => void
  step?: number
}) {
  return (
    <Field label={label}>
      <input
        type="number"
        step={step}
        value={value}
        onChange={(e) => onChange(Number(e.target.value))}
        className={`${input} font-mono`}
      />
    </Field>
  )
}

function Check({
  label,
  checked,
  onChange,
}: {
  label: string
  checked: boolean
  onChange: (v: boolean) => void
}) {
  return (
    <label className="flex items-center gap-2 text-sm text-slate-300">
      <input
        type="checkbox"
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
        className="h-4 w-4 rounded border-slate-600 bg-slate-900"
      />
      {label}
    </label>
  )
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="space-y-4 rounded-lg bg-slate-900/30 p-4 ring-1 ring-slate-800/60">
      <h2 className="text-[11px] font-medium uppercase tracking-[0.08em] text-slate-500">
        {title}
      </h2>
      {children}
    </section>
  )
}

/// The resume, uploaded separately from the rest of the settings.
///
/// It arrives as a file and is extracted server-side, so it cannot ride along
/// in the JSON save — and it is the single biggest lever on match quality, so
/// it gets its own block at the top rather than being buried among the sliders.
function ResumeBlock({ s, reload }: { s: Settings; reload: () => void }) {
  const [busy, setBusy] = useState(false)
  const [note, setNote] = useState<{ text: string; ok: boolean } | null>(null)
  const fileRef = useRef<HTMLInputElement>(null)
  const [pasted, setPasted] = useState('')

  const upload = async (form: FormData) => {
    setBusy(true)
    setNote(null)
    try {
      const res = await fetch('/api/settings/resume', { method: 'POST', body: form })
      const body = await res.json()
      setNote({ text: body.message ?? 'Done.', ok: !!body.ok })
      if (body.ok) {
        setPasted('')
        if (fileRef.current) fileRef.current.value = ''
        reload()
      }
    } catch (e) {
      setNote({ text: e instanceof Error ? e.message : 'Upload failed', ok: false })
    } finally {
      setBusy(false)
    }
  }

  const clear = async () => {
    setBusy(true)
    try {
      const res = await fetch('/api/settings/resume/clear', { method: 'POST' })
      const body = await res.json()
      setNote({ text: body.message, ok: true })
      reload()
    } catch (e) {
      setNote({ text: e instanceof Error ? e.message : 'Failed', ok: false })
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="space-y-3 rounded-lg bg-slate-900/40 p-4 ring-1 ring-slate-800">
      <div className="flex items-center justify-between">
        <h2 className="text-[11px] font-medium uppercase tracking-[0.08em] text-slate-500">
          Resume
        </h2>
        {s.has_resume && (
          <button
            onClick={clear}
            disabled={busy}
            className="text-xs text-slate-500 hover:text-rose-300 disabled:opacity-50"
          >
            remove
          </button>
        )}
      </div>

      {/* The ATS scores against what was *read*, not against the PDF. If it
          read you as a frontend engineer with two years then every score on
          the board is wrong, and without showing this you'd have no way to
          find that out. */}
      {s.has_resume && (
        <dl className="grid grid-cols-2 gap-x-4 gap-y-1 rounded-md bg-slate-950/40 p-3 text-xs sm:grid-cols-4">
          {[
            ['experience', s.profile.years ? `${s.profile.years} years` : 'not set'],
            ['reads as', s.profile.level ?? 'unclear'],
            ['discipline', s.profile.roles.join(', ') || 'unclear'],
            ['stack', `${s.profile.stack.length} technologies`],
          ].map(([k, v]) => (
            <div key={k}>
              <dt className="text-[10px] uppercase tracking-[0.08em] text-slate-600">{k}</dt>
              <dd className="text-slate-300">{v}</dd>
            </div>
          ))}
          {s.profile.stack.length > 0 && (
            <div className="col-span-2 sm:col-span-4">
              <dt className="sr-only">detected stack</dt>
              <dd className="font-mono text-[11px] leading-relaxed text-slate-600">
                {s.profile.stack.join(', ')}
              </dd>
            </div>
          )}
        </dl>
      )}

      <p className="text-sm text-slate-400">
        {s.has_resume
          ? `Loaded${s.resume_filename ? ` from ${s.resume_filename}` : ''} — ${s.resume_chars.toLocaleString()} characters. Similarity scoring is on.`
          : 'No resume loaded. Scoring is using your keyword list alone, which is a much blunter instrument.'}
      </p>

      <div className="flex flex-wrap items-center gap-2">
        <input
          ref={fileRef}
          type="file"
          accept=".pdf,.txt,.md,text/plain,application/pdf"
          className="text-xs text-slate-400 file:mr-2 file:rounded-md file:border-0 file:bg-slate-700 file:px-3 file:py-1.5 file:text-xs file:text-slate-100"
        />
        <button
          disabled={busy}
          onClick={() => {
            const f = fileRef.current?.files?.[0]
            if (!f) return setNote({ text: 'Choose a file first.', ok: false })
            const fd = new FormData()
            fd.append('file', f)
            upload(fd)
          }}
          className="rounded-md bg-slate-700 px-3 py-1.5 text-xs text-slate-100 hover:bg-slate-600 disabled:opacity-50"
        >
          Upload
        </button>
      </div>

      <details className="text-xs text-slate-500">
        <summary className="cursor-pointer hover:text-slate-300">or paste the text</summary>
        <textarea
          rows={6}
          value={pasted}
          onChange={(e) => setPasted(e.target.value)}
          placeholder="Paste your resume here — useful for .docx, or a scanned PDF with no text layer."
          className={`${input} mt-2 font-mono`}
        />
        <button
          disabled={busy || pasted.trim().length < 100}
          onClick={() => {
            const fd = new FormData()
            fd.append('text', pasted)
            upload(fd)
          }}
          className="mt-2 rounded-md bg-slate-700 px-3 py-1.5 text-xs text-slate-100 hover:bg-slate-600 disabled:opacity-50"
        >
          Use pasted text
        </button>
      </details>

      {note && (
        <p className={`text-xs ${note.ok ? 'text-emerald-400' : 'text-rose-400'}`}>{note.text}</p>
      )}
    </div>
  )
}

export function SettingsPanel() {
  const [s, setS] = useState<Settings | null>(null)
  const [err, setErr] = useState<string | null>(null)
  const [saving, setSaving] = useState(false)
  const [saved, setSaved] = useState<string | null>(null)

  const load = () => {
    getSettings()
      .then(setS)
      .catch((e) => setErr(e.message))
  }
  useEffect(load, [])

  if (err) return <p className="text-sm text-rose-400">Couldn't load settings — {err}</p>
  if (!s) return <p className="text-sm text-slate-500">Loading…</p>

  const set = <K extends keyof Settings>(k: K, v: Settings[K]) => setS({ ...s, [k]: v })

  const save = async () => {
    setSaving(true)
    setSaved(null)
    try {
      // The server clamps and returns what it stored, so the form is repopulated
      // from the response — otherwise it shows a floor of 90 that the engine is
      // quietly treating as something else.
      const stored = await putSettings(s)
      setS(stored)
      setSaved('Saved.')
    } catch (e) {
      setSaved(e instanceof Error ? `Save failed — ${e.message}` : 'Save failed')
    } finally {
      setSaving(false)
    }
  }

  return (
    <div className="space-y-4 pb-16">
      <ResumeBlock s={s} reload={load} />

      <Section title="What you're looking for">
        <ListField
          label="Target titles"
          hint="One per line. Matched against the job title; a full match is the strongest single signal."
          value={s.titles}
          rows={5}
          onChange={(v) => set('titles', v)}
        />
        <ListField
          label="Skills / keywords"
          hint="One per line. Coverage across the post body."
          value={s.keywords}
          rows={5}
          onChange={(v) => set('keywords', v)}
        />
        <ListField
          label="Locations"
          hint="One per line. A country matches its cities, so 'india' covers Bengaluru and Gurugram."
          value={s.locations}
          rows={3}
          onChange={(v) => set('locations', v)}
        />
        <ListField
          label="Seniority"
          hint="One per line, e.g. senior, sde 2. Junior titles are penalised when you want senior."
          value={s.seniority}
          rows={3}
          onChange={(v) => set('seniority', v)}
        />
        <Field
          label="Your experience, in years"
          hint="The single biggest lever on the score. Typed rather than guessed from your resume, because you know it exactly — and a posting asking for four more years than you have is a reach however well the stack matches."
        >
          <input
            type="number"
            min={0}
            max={50}
            value={s.years_experience ?? ''}
            onChange={(e) =>
              set('years_experience', e.target.value === '' ? null : Number(e.target.value))
            }
            className={`${input} font-mono`}
          />
        </Field>

        <ListField
          label="Languages you write"
          hint="One per line: go, python, rust. This is a gate, not a bonus — see below."
          value={s.stack}
          rows={4}
          onChange={(v) => set('stack', v)}
        />
        <Field
          label="How much the stack counts"
          hint="A senior backend role in your city banks 62 points for its title, location and level before anything asks what language it's in — which is why a Ruby job scores in the seventies and lands on your board. This is what asks. A listing that names no language at all is never judged either way."
        >
          <select
            value={s.stack_policy}
            onChange={(e) => set('stack_policy', e.target.value as Settings['stack_policy'])}
            className={input}
          >
            <option value="off">off — any language</option>
            <option value="prefer">prefer — penalise other languages heavily</option>
            <option value="require">require — discard them outright</option>
          </select>
        </Field>

        <ListField
          label="Tell me the moment you see"
          hint="One technology per line. A posting naming one of these skips the score floor, the settling window and the release bar — it appears on the board and goes out the moment it is found, whatever else it scored. The hourly cap still holds. This is a standing alert, not a filter: it never hides anything."
          value={s.instant_stack}
          rows={2}
          onChange={(v) => set('instant_stack', v)}
        />

        <ListField
          label="Dealbreakers"
          hint="One per line. Any hit zeroes the post outright — keep these specific."
          value={s.dealbreakers}
          rows={3}
          onChange={(v) => set('dealbreakers', v)}
        />

        <Field
          label="How much location counts"
          hint="'prefer' is worth a few points, which a strong match elsewhere outweighs. 'require' is a gate: anywhere else is discarded."
        >
          <select
            value={s.location_policy}
            onChange={(e) => set('location_policy', e.target.value as Settings['location_policy'])}
            className={input}
          >
            <option value="off">off — ignore location</option>
            <option value="prefer">prefer — a few points</option>
            <option value="require">require — discard anywhere else</option>
          </select>
        </Field>

        <div className="space-y-2">
          <Check
            label="Remote roles count as a location match"
            checked={s.remote_ok}
            onChange={(v) => set('remote_ok', v)}
          />
          <Check
            label="Keep posts whose location can't be determined"
            checked={s.allow_unknown_location}
            onChange={(v) => set('allow_unknown_location', v)}
          />
        </div>

        <Field
          label="How much the resume counts"
          hint={`${Math.round(s.resume_weight * 100)}% resume similarity, ${Math.round((1 - s.resume_weight) * 100)}% keyword rules.`}
        >
          <input
            type="range"
            min={0}
            max={1}
            step={0.05}
            value={s.resume_weight}
            onChange={(e) => set('resume_weight', Number(e.target.value))}
            className="w-full"
          />
        </Field>
      </Section>

      <Section title="Sources">
        <Field
          label="Mode"
          hint="Anything but 'custom' drives the individual toggles, so the mode is always the truth."
        >
          <select value={s.mode} onChange={(e) => set('mode', e.target.value)} className={input}>
            <option value="all">all — boards and feed posts</option>
            <option value="boards">boards only — Greenhouse and Workday</option>
            <option value="posts">posts only — the LinkedIn feed</option>
            <option value="custom">custom — the toggles below</option>
          </select>
        </Field>

        <div className="space-y-3">
          <Check
            label="Greenhouse boards"
            checked={s.greenhouse_enabled}
            onChange={(v) => set('greenhouse_enabled', v)}
          />
          <Check
            label="Lever boards"
            checked={s.lever_enabled}
            onChange={(v) => set('lever_enabled', v)}
          />
          <Check
            label="Workday careers sites"
            checked={s.workday_enabled}
            onChange={(v) => set('workday_enabled', v)}
          />
          <Check
            label="LinkedIn guest jobs"
            checked={s.linkedin_guest_enabled}
            onChange={(v) => set('linkedin_guest_enabled', v)}
          />
          <Check
            label="LinkedIn feed posts — authenticated, fragile, ban risk"
            checked={s.voyager_enabled}
            onChange={(v) => set('voyager_enabled', v)}
          />
        </div>

        {s.company_lists.map((l) => (
          <CompanyListView key={l.kind} list={l} />
        ))}

        <Num
          label="Ignore Workday listings older than (days)"
          value={s.workday_lookback_days}
          onChange={(v) => set('workday_lookback_days', v)}
        />

        <ListField
          label="Feed searches"
          hint="One per line. Hashtags work best: #hiring, #hiringnow. Each runs as its own search."
          value={s.voyager_queries}
          rows={4}
          onChange={(v) => set('voyager_queries', v)}
        />
        <Field
          label="Voyager queryId"
          hint="Not a secret — a constant that breaks whenever LinkedIn ships. Copy it from DevTools → Network on a content search."
        >
          <input
            value={s.voyager_query_id}
            onChange={(e) => set('voyager_query_id', e.target.value)}
            placeholder="voyagerSearchDashClusters.xxxxxxxx"
            className={`${input} font-mono`}
          />
        </Field>
        <div className="grid grid-cols-3 gap-3">
          <Num
            label="Collect posts from (h)"
            value={s.lookback_hours}
            onChange={(v) => set('lookback_hours', v)}
          />
          <Num
            label="Pages on first fill"
            value={s.voyager_max_pages}
            onChange={(v) => set('voyager_max_pages', v)}
          />
          <Num
            label="Pages when polling"
            value={s.voyager_poll_pages}
            onChange={(v) => set('voyager_poll_pages', v)}
          />
        </div>
        <p className="text-xs text-slate-500">
          Your <code className="text-slate-400">li_at</code> cookie stays in{' '}
          <code className="text-slate-400">.env</code> — secrets don't belong in a web form.
        </p>
      </Section>

      <Section title="Alert budget & tiers">
        <div className="grid grid-cols-2 gap-3 sm:grid-cols-3">
          <Num label="Alerts / hour" value={s.per_hour_cap} onChange={(v) => set('per_hour_cap', v)} />
          <Num
            label="Per company / hour"
            value={s.per_poster_cap}
            onChange={(v) => set('per_poster_cap', v)}
          />
          <Num label="Radar window (h)" value={s.radar_hours} onChange={(v) => set('radar_hours', v)} />
          <Num label="Floor (drop below)" value={s.score_floor} onChange={(v) => set('score_floor', v)} />
          <Num label="Strong at" value={s.strong_min} onChange={(v) => set('strong_min', v)} />
          <Num
            label="Exceptional at"
            value={s.exceptional_min}
            onChange={(v) => set('exceptional_min', v)}
          />
          <Num
            label="Settle strong (min)"
            value={Math.round(s.settle_strong_secs / 60)}
            onChange={(v) => set('settle_strong_secs', v * 60)}
          />
          <Num
            label="Keep competing (h)"
            value={Math.round(s.candidate_ttl_secs / 3600)}
            onChange={(v) => set('candidate_ttl_secs', v * 3600)}
          />
          <Num label="Outbox bar" value={s.outbox_min} onChange={(v) => set('outbox_min', v)} />
        </div>

        <Check
          label="Move the bar with the remaining budget"
          checked={s.adaptive_threshold}
          onChange={(v) => set('adaptive_threshold', v)}
        />
        <div className="grid grid-cols-2 gap-3">
          {/* The last slot of the hour is the expensive one — spend it on
              something mediocre and the good post twenty minutes later waits.
              The first is cheap, and not spending it wastes the hour. */}
          <Num
            label="Bar on the last slot"
            value={s.adaptive_start}
            onChange={(v) => set('adaptive_start', v)}
          />
          <Num
            label="Bar with all slots free"
            value={s.adaptive_end}
            onChange={(v) => set('adaptive_end', v)}
          />
        </div>
        <p className="text-xs text-slate-500">
          Values are clamped on save so the ladder can't invert: floor &lt; strong &lt; exceptional,
          and the per-company cap can't exceed the hourly one. What you see after saving is what was
          actually stored.
        </p>
      </Section>

      {/* Sticky, because this form is long enough that the button would
          otherwise be a scroll away from whatever you just changed. */}
      <div className="sticky bottom-0 -mx-4 flex items-center gap-3 border-t border-slate-800 bg-slate-950/95 px-4 py-3 backdrop-blur">
        <button
          onClick={save}
          disabled={saving}
          className="rounded-md bg-emerald-500/90 px-4 py-2 text-sm font-medium text-slate-900 hover:bg-emerald-400 disabled:opacity-50"
        >
          {saving ? 'Saving…' : 'Save settings'}
        </button>
        {saved && (
          <span
            className={`text-xs ${saved.startsWith('Save failed') ? 'text-rose-400' : 'text-emerald-400'}`}
          >
            {saved}
          </span>
        )}
      </div>
    </div>
  )
}
