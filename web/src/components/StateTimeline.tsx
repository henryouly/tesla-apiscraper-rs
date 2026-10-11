import { For, Show, createMemo } from 'solid-js'
import type { StatePoint } from '../lib/dashboards'

/** Fill classes per known state (lowercased); dark-mode aware via `dark:` variants. */
const STATE_FILLS: Record<string, string> = {
  online: 'fill-emerald-500 dark:fill-emerald-400',
  driving: 'fill-blue-500 dark:fill-blue-400',
  charging: 'fill-amber-500 dark:fill-amber-400',
  asleep: 'fill-indigo-500 dark:fill-indigo-400',
  offline: 'fill-zinc-400 dark:fill-zinc-500',
  updating: 'fill-violet-500 dark:fill-violet-400',
}

const FALLBACK_FILL = 'fill-zinc-300 dark:fill-zinc-600'

function fillFor(state: string): string {
  return STATE_FILLS[state.toLowerCase()] ?? FALLBACK_FILL
}

/** Dot swatch for the legend: same hues as the SVG fills, as backgrounds. */
function dotFor(state: string): string {
  return fillFor(state).replaceAll('fill-', 'bg-')
}

interface Segment {
  state: string
  /** Percentage offsets within [0, 100]. */
  x: number
  w: number
  tooltip: string
}

function fmtT(t: number): string {
  const d = new Date(t)
  return Number.isNaN(d.getTime()) ? '—' : d.toLocaleString()
}

export function StateTimeline(props: { data: Array<StatePoint>; from: number; to: number }) {
  const segments = createMemo((): Array<Segment> => {
    const { from, to } = props
    if (!Number.isFinite(from) || !Number.isFinite(to) || to <= from) return []
    const pts = [...props.data].sort((a, b) => a.t - b.t)
    if (pts.length === 0) return []
    const span = to - from
    const segs: Array<Segment> = []
    for (let i = 0; i < pts.length; i++) {
      const p = pts[i]
      const next = pts[i + 1]
      const start = i === 0 ? Math.max(from, p.t) : p.t
      const end = next ? next.t : to
      const cs = Math.min(Math.max(start, from), to)
      const ce = Math.min(Math.max(end, from), to)
      if (ce <= cs) continue
      segs.push({
        state: p.state,
        x: ((cs - from) / span) * 100,
        w: ((ce - cs) / span) * 100,
        tooltip: `${p.state} — ${fmtT(cs)} → ${fmtT(ce)}`,
      })
    }
    return segs
  })

  const legend = createMemo((): Array<string> => {
    const seen: Array<string> = []
    for (const p of props.data) {
      if (!seen.includes(p.state)) seen.push(p.state)
    }
    return seen
  })

  return (
    <Show
      when={segments().length > 0}
      fallback={<p class="py-6 text-center text-sm text-zinc-500 dark:text-zinc-400">No state changes in this period.</p>}
    >
      <div>
        <svg
          viewBox="0 0 100 10"
          preserveAspectRatio="none"
          class="block h-10 w-full overflow-hidden rounded-lg"
          role="img"
          aria-label="Vehicle state timeline"
        >
          <For each={segments()}>
            {(s) => (
              <rect x={s.x} y={0} width={s.w} height={10} class={fillFor(s.state)}>
                <title>{s.tooltip}</title>
              </rect>
            )}
          </For>
        </svg>
        <div class="mt-3 flex flex-wrap gap-x-4 gap-y-1.5">
          <For each={legend()}>
            {(state) => (
              <span class="inline-flex items-center gap-1.5 text-xs text-zinc-600 dark:text-zinc-300">
                <span class={`inline-block h-2.5 w-2.5 rounded-full ${dotFor(state)}`} aria-hidden="true" />
                {state}
              </span>
            )}
          </For>
        </div>
      </div>
    </Show>
  )
}
