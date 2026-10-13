import { For, Show, createEffect, onCleanup } from 'solid-js'
import uPlot from 'uplot'
import 'uplot/dist/uPlot.min.css'
import { useTheme } from './theme'

export interface ChartSeries {
  label: string
  /** Epoch-millisecond [x, y] pairs. Null y values render as gaps. */
  data: Array<[number, number | null]>
  color?: string
}

interface TimeChartProps {
  series: Array<ChartSeries>
  height?: number
  yUnit?: string
  /** Optional explicit x-domain (epoch millis). When omitted uPlot fits the data. */
  xMin?: number
  xMax?: number
  /**
   * Optional uPlot cursor sync key. Charts sharing a key move their
   * crosshairs together (matches Grafana `graphTooltip: 1`). When omitted
   * each chart keeps an independent cursor.
   */
  syncKey?: string
}

const PALETTE = ['#e82127', '#0ea5e9', '#10b981', '#f59e0b', '#8b5cf6', '#ec4899']

function seriesColor(s: ChartSeries, i: number): string {
  return s.color ?? PALETTE[i % PALETTE.length]
}

/** Union all series onto a shared second-resolution x grid (uPlot wants seconds). */
function toAligned(input: Array<ChartSeries>): { xs: Array<number>; ys: Array<Array<number | null>> } {
  const xSet = new Set<number>()
  for (const s of input) {
    for (const [t] of s.data) xSet.add(Math.round(t / 1000))
  }
  const xs = [...xSet].sort((a, b) => a - b)
  const ys = input.map((s) => {
    const m = new Map<number, number | null>()
    for (const [t, v] of s.data) m.set(Math.round(t / 1000), v)
    return xs.map((x) => m.get(x) ?? null)
  })
  return { xs, ys }
}

export function TimeChart(props: TimeChartProps) {
  const { dark } = useTheme()
  let host: HTMLDivElement | undefined
  let plot: uPlot | undefined
  let ro: ResizeObserver | undefined

  const hasData = () => props.series.some((s) => s.data.length > 0)
  const height = () => props.height ?? 260

  createEffect(() => {
    const isDark = dark()
    const series = props.series
    const h = height()
    const label = props.yUnit
    const syncKey = props.syncKey
    const ready = hasData()
    plot?.destroy()
    plot = undefined
    ro?.disconnect()
    ro = undefined
    if (!ready || host == null) return
    const el = host
    const { xs, ys } = toAligned(series)
    if (xs.length === 0) return
    // Zinc palette, low-contrast grid in both themes.
    const grid = isDark ? 'rgba(255,255,255,0.08)' : 'rgba(24,24,27,0.08)'
    const ink = isDark ? '#a1a1aa' : '#71717a'
    const data = [xs, ...ys] as uPlot.AlignedData
    const opts: uPlot.Options = {
      width: el.clientWidth || 640,
      height: h,
      legend: { show: false },
      cursor: { show: true, x: true, y: true, sync: syncKey != null ? { key: syncKey } : undefined },
      scales: {
        x: {
          time: true,
          min: props.xMin != null ? props.xMin / 1000 : undefined,
          max: props.xMax != null ? props.xMax / 1000 : undefined,
        },
      },
      series: [
        { label: 'Time' },
        ...series.map(
          (s, i): uPlot.Series => ({
            label: s.label,
            stroke: seriesColor(s, i),
            width: 2,
            points: { show: false },
            spanGaps: false,
          }),
        ),
      ],
      axes: [
        {
          stroke: ink,
          grid: { show: true, stroke: grid, width: 1 },
          ticks: { show: true, stroke: grid },
          font: '11px system-ui, sans-serif',
        },
        {
          stroke: ink,
          label,
          grid: { show: true, stroke: grid, width: 1 },
          ticks: { show: true, stroke: grid },
          font: '11px system-ui, sans-serif',
          size: 56,
        },
      ],
    }
    const current = new uPlot(opts, data, el)
    plot = current
    ro = new ResizeObserver(() => {
      current.setSize({ width: el.clientWidth || 640, height: h })
    })
    ro.observe(el)
  })

  onCleanup(() => {
    ro?.disconnect()
    plot?.destroy()
    plot = undefined
  })

  return (
    <div>
      <Show
        when={hasData()}
        fallback={<p class="py-8 text-center text-sm text-zinc-500 dark:text-zinc-400">No data for this period.</p>}
      >
        <div
          ref={(el) => {
            host = el
          }}
          class="w-full"
        />
        <div class="mt-2 flex flex-wrap gap-x-4 gap-y-1">
          <For each={props.series}>
            {(s, i) => (
              <span class="inline-flex items-center gap-1.5 text-xs text-zinc-500 dark:text-zinc-400">
                <span class="h-2 w-2 rounded-full" style={{ background: seriesColor(s, i()) }} />
                {s.label}
              </span>
            )}
          </For>
        </div>
      </Show>
    </div>
  )
}
