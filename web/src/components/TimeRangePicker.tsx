import { For, Show, createMemo, createSignal } from 'solid-js'
import type { Range } from '../lib/dashboards'
import { Alert } from './ui'

interface Preset {
  label: string
  spanMs: number
}

const PRESETS: Array<Preset> = [
  { label: 'Last 5 minutes', spanMs: 5 * 60 * 1000 },
  { label: 'Last 15 minutes', spanMs: 15 * 60 * 1000 },
  { label: 'Last 1 hour', spanMs: 60 * 60 * 1000 },
  { label: 'Last 3 hours', spanMs: 3 * 60 * 60 * 1000 },
  { label: 'Last 6 hours', spanMs: 6 * 60 * 60 * 1000 },
  { label: 'Last 12 hours', spanMs: 12 * 60 * 60 * 1000 },
  { label: 'Last 24 hours', spanMs: 24 * 60 * 60 * 1000 },
  { label: 'Last 2 days', spanMs: 2 * 24 * 60 * 60 * 1000 },
  { label: 'Last 7 days', spanMs: 7 * 24 * 60 * 60 * 1000 },
  { label: 'Last 30 days', spanMs: 30 * 24 * 60 * 60 * 1000 },
]

/** Tolerance for matching a range against a preset: both the span and `to`-vs-now must be within this. */
const MATCH_TOLERANCE_MS = 10_000

export function matchPreset(range: Range, nowMs: number): Preset | null {
  const toMs = Date.parse(range.from) <= Date.parse(range.to) ? Date.parse(range.to) : NaN
  if (Number.isNaN(toMs)) return null
  if (Math.abs(nowMs - toMs) > MATCH_TOLERANCE_MS) return null
  const spanMs = toMs - Date.parse(range.from)
  return PRESETS.find((p) => Math.abs(spanMs - p.spanMs) <= MATCH_TOLERANCE_MS) ?? null
}

function formatCustomEndpoint(iso: string): string {
  const d = new Date(iso)
  return d.toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' })
}

export function formatCustomLabel(range: Range): string {
  return `${formatCustomEndpoint(range.from)} – ${formatCustomEndpoint(range.to)}`
}

/** Format an ISO timestamp as a `datetime-local` input value in local time. */
function toLocalInputValue(iso: string): string {
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return ''
  const pad = (n: number): string => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`
}

const LABEL_CLASSES = 'mb-1.5 block text-[13px] font-medium text-zinc-700 dark:text-zinc-300'
const INPUT_CLASSES =
  'w-full rounded-xl border border-zinc-300 bg-white px-3 py-2 text-sm text-zinc-900 transition-colors focus:border-[#e82127]/60 focus:outline-none focus:ring-2 focus:ring-[#e82127]/20 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-red-500 dark:border-white/10 dark:bg-white/[0.04] dark:text-zinc-100'

export function TimeRangePicker(props: { value: Range; onChange: (r: Range) => void }) {
  const [open, setOpen] = createSignal(false)
  const [fromInput, setFromInput] = createSignal('')
  const [toInput, setToInput] = createSignal('')
  const [error, setError] = createSignal<string | null>(null)

  const openPanel = (): void => {
    setFromInput(toLocalInputValue(props.value.from))
    setToInput(toLocalInputValue(props.value.to))
    setError(null)
    setOpen(true)
  }

  const label = createMemo((): string => {
    const preset = matchPreset(props.value, Date.now())
    if (preset) return preset.label
    return formatCustomLabel(props.value)
  })

  const activePreset = createMemo((): string | null => {
    const preset = matchPreset(props.value, Date.now())
    return preset ? preset.label : null
  })

  const applyPreset = (preset: Preset): void => {
    const to = new Date()
    const from = new Date(to.getTime() - preset.spanMs)
    props.onChange({ from: from.toISOString(), to: to.toISOString() })
    setOpen(false)
  }

  const applyCustom = (): void => {
    const fromMs = new Date(fromInput()).getTime()
    const toMs = new Date(toInput()).getTime()
    if (Number.isNaN(fromMs) || Number.isNaN(toMs) || !(fromMs < toMs)) {
      setError('Enter a valid range with From before To.')
      return
    }
    setError(null)
    props.onChange({ from: new Date(fromInput()).toISOString(), to: new Date(toInput()).toISOString() })
    setOpen(false)
  }

  return (
    <div class="relative">
      <span class={LABEL_CLASSES}>Time range</span>
      <button
        type="button"
        aria-haspopup="dialog"
        aria-expanded={open()}
        onClick={() => (open() ? setOpen(false) : openPanel())}
        class="inline-flex w-full items-center justify-between gap-1.5 rounded-xl border border-zinc-300 bg-white px-3 py-2 text-sm font-medium text-zinc-700 transition-colors hover:bg-zinc-100 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-red-500 dark:border-white/10 dark:bg-white/[0.06] dark:text-zinc-100 dark:hover:bg-white/[0.1]"
      >
        <span class="truncate">{label()}</span>
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" class="h-4 w-4 shrink-0" aria-hidden="true">
          <path d="M6 9l6 6 6-6" />
        </svg>
      </button>
      <Show when={open()}>
        <button type="button" aria-label="Close time range picker" onClick={() => setOpen(false)} class="fixed inset-0 z-40 cursor-default bg-transparent" />
        <div role="dialog" aria-label="Time range" class="absolute right-0 z-50 mt-2 w-72 rounded-2xl border border-zinc-200 bg-white p-3 shadow-lg dark:border-white/[0.07] dark:bg-[#101013]">
          <div class="flex flex-col gap-1">
            <For each={PRESETS}>
              {(preset) => (
                <button
                  type="button"
                  onClick={() => applyPreset(preset)}
                  aria-pressed={activePreset() === preset.label}
                  class={`rounded-lg px-2.5 py-1.5 text-left text-sm transition-colors focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-red-500 ${
                    activePreset() === preset.label
                      ? 'bg-zinc-900/[0.06] font-semibold text-zinc-900 dark:bg-white/[0.1] dark:text-zinc-50'
                      : 'text-zinc-600 hover:bg-zinc-900/[0.04] dark:text-zinc-300 dark:hover:bg-white/[0.06]'
                  }`}
                >
                  {preset.label}
                </button>
              )}
            </For>
          </div>
          <div class="mt-3 border-t border-zinc-200 pt-3 dark:border-white/[0.07]">
            <div class="grid gap-2">
              <label class="block">
                <span class={LABEL_CLASSES}>From</span>
                <input type="datetime-local" value={fromInput()} onInput={(e) => setFromInput(e.currentTarget.value)} class={INPUT_CLASSES} />
              </label>
              <label class="block">
                <span class={LABEL_CLASSES}>To</span>
                <input type="datetime-local" value={toInput()} onInput={(e) => setToInput(e.currentTarget.value)} class={INPUT_CLASSES} />
              </label>
            </div>
            <Show when={error()}>
              <div class="mt-2 text-xs">
                <Alert tone="error">{error() ?? ''}</Alert>
              </div>
            </Show>
            <button
              type="button"
              onClick={applyCustom}
              class="mt-2 inline-flex w-full items-center justify-center gap-1.5 rounded-lg bg-[#e82127] px-3.5 py-2 text-sm font-medium text-white shadow-[0_8px_20px_-8px_rgba(232,33,39,0.7)] transition-all hover:bg-[#c81a20] focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-red-500 active:scale-[0.98]"
            >
              Apply
            </button>
          </div>
        </div>
      </Show>
    </div>
  )
}
