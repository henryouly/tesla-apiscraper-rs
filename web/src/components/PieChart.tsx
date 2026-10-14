import { For, Show, createMemo } from 'solid-js'

export interface PieSlice {
  label: string
  value: number
  color: string
}

/** Tiny SVG pie chart (single use: the Trip page Time Spent panel).
 * Presentational only — no chart lib. Zero-total renders a muted ring. */
export function PieChart(props: { slices: Array<PieSlice>; size?: number }) {
  const size = () => props.size ?? 180
  const total = createMemo(() => props.slices.reduce((acc, s) => acc + Math.max(0, s.value), 0))

  const arcs = createMemo(() => {
    const r = 80
    const cx = 100
    const cy = 100
    const t = total()
    if (t <= 0) return []
    let acc = 0
    return props.slices.map((s) => {
      const frac = Math.max(0, s.value) / t
      const start = (acc / t) * Math.PI * 2 - Math.PI / 2
      acc += Math.max(0, s.value)
      const end = (acc / t) * Math.PI * 2 - Math.PI / 2
      const large = frac > 0.5 ? 1 : 0
      const x1 = cx + r * Math.cos(start)
      const y1 = cy + r * Math.sin(start)
      const x2 = cx + r * Math.cos(end)
      const y2 = cy + r * Math.sin(end)
      // A full circle cannot be drawn with one arc — use two halves.
      const d =
        frac >= 1
          ? `M ${cx - r} ${cy} A ${r} ${r} 0 1 0 ${cx + r} ${cy} A ${r} ${r} 0 1 0 ${cx - r} ${cy} Z`
          : `M ${cx} ${cy} L ${x1} ${y1} A ${r} ${r} 0 ${large} 1 ${x2} ${y2} Z`
      return { label: s.label, value: s.value, color: s.color, d }
    })
  })

  const pct = (v: number) => {
    const t = total()
    return t <= 0 ? '—' : `${Math.round((Math.max(0, v) / t) * 100)}%`
  }

  return (
    <div class="flex flex-col items-center gap-4 sm:flex-row sm:justify-center sm:gap-8">
      <svg
        width={size()}
        height={size()}
        viewBox="0 0 200 200"
        role="img"
        aria-label="Time spent breakdown"
        class="shrink-0"
      >
        <Show
          when={total() > 0}
          fallback={<circle cx="100" cy="100" r="80" class="fill-zinc-200 dark:fill-white/10" />}
        >
          <For each={arcs()}>
            {(a) => (
              <path d={a.d} fill={a.color}>
                <title>{`${a.label}: ${pct(a.value)}`}</title>
              </path>
            )}
          </For>
        </Show>
      </svg>
      <ul class="flex flex-col gap-2 text-sm">
        <For each={props.slices}>
          {(s) => (
            <li class="flex items-center gap-2 text-zinc-700 dark:text-zinc-200">
              <span class="inline-block h-3 w-3 rounded-sm" style={{ 'background-color': s.color }} />
              <span class="font-medium">{s.label}</span>
              <span class="tabular-nums text-zinc-500 dark:text-zinc-400">{pct(s.value)}</span>
            </li>
          )}
        </For>
      </ul>
    </div>
  )
}
