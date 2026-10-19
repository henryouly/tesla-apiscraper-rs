import { Show, createEffect, onCleanup } from 'solid-js'
import uPlot from 'uplot'
import 'uplot/dist/uPlot.min.css'
import { useTheme } from './theme'

export interface ChartSeries {
  label: string
  /** Epoch-millisecond [x, y] pairs. Null y values render as gaps. */
  data: Array<[number, number | null]>
  color?: string
  /**
   * Appended to the legend/tooltip value, for charts whose series carry
   * different units (the Drive chart mixes mph, kW, % and km on one axis).
   * Omitted → uPlot's own value formatting, unchanged.
   */
  unit?: string
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
  /**
   * Fill the available height instead of using a fixed pixel height: the plot
   * takes the host's box minus whatever the legend occupies, measured from the
   * rendered legend. Lets a grid declare row heights without every page
   * hard-coding a legend height. Falls back to `height` if the box is
   * degenerate.
   */
  fill?: boolean
}

const PALETTE = ['#e82127', '#0ea5e9', '#10b981', '#f59e0b', '#8b5cf6', '#ec4899']

function seriesColor(s: ChartSeries, i: number): string {
  return s.color ?? PALETTE[i % PALETTE.length]
}

/**
 * Value text for a series that declares a unit. Mirrors uPlot's own default
 * (`numSeriesVal`): `'--'` when the cursor is off the plot, empty when the
 * point is missing — so the legend keeps the same sentinel the floats
 * unfloated. One decimal, because uPlot's default Intl formatting prints full
 * precision (55.234987), which is unreadable in a tooltip.
 */
function unitValue(unit: string): uPlot.Series.Value {
  return (_self, raw, _seriesIdx, idx) => {
    if (idx == null) return '--'
    if (raw == null) return ''
    return `${raw.toLocaleString(undefined, { maximumFractionDigits: 1 })} ${unit}`
  }
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

// Below this a fill-mode chart is collapsed or unmeasured; rendering nothing
// is worse than falling back to the caller's height.
const MIN_FILL_H = 80

export function TimeChart(props: TimeChartProps) {
  const { dark } = useTheme()
  let host: HTMLDivElement | undefined
  let plot: uPlot | undefined
  let ro: ResizeObserver | undefined

  const hasData = () => props.series.some((s) => s.data.length > 0)
  const fallbackHeight = () => props.height ?? 260
  /**
   * Plot height. In fill mode the host's own box minus the legend it rendered,
   * so the legend's height stays a property of the chart (and of the legend's
   * CSS) instead of a constant each page has to agree on. A floated legend is
   * out of flow and contributes nothing, so it is not subtracted.
   */
  const plotHeight = () => {
    if (props.fill !== true || host == null) return fallbackHeight()
    const legend = host.querySelector<HTMLElement>('.u-legend')
    const inFlow = legend != null && getComputedStyle(legend).position !== 'absolute'
    const measured = host.clientHeight - (inFlow ? (legend?.offsetHeight ?? 0) : 0)
    return measured >= MIN_FILL_H ? measured : fallbackHeight()
  }

  createEffect(() => {
    const isDark = dark()
    const series = props.series
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
    // uPlot has no theme option, so both axes get the same ink/grid colours —
    // that is the only reason they are configured at all. Font, tick shape and
    // axis lines are left to uPlot's defaults.
    const axisTheme = {
      stroke: ink,
      grid: { show: true, stroke: grid, width: 1 },
      ticks: { show: true, stroke: grid },
    }
    const data = [xs, ...ys] as uPlot.AlignedData
    const opts: uPlot.Options = {
      width: el.clientWidth || 640,
      height: plotHeight(),
      // uPlot's own legend, floated over the plot as a value card and revealed
      // on hover by the `.u-legend` rules in `index.css`. `live` defaults to
      // true, so the values track the crosshair without being asked.
      legend: { show: true },
      // The crosshair defaults to both axes on; only the sync key is ours.
      cursor: { sync: syncKey != null ? { key: syncKey } : undefined },
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
            // 2px over uPlot's 1px default, to read on a laptop panel. Points
            // stay off: uPlot auto-shows them on sparse series (the per-day
            // charts), and the lines carry the values.
            width: 2,
            points: { show: false },
            // No unit → leave uPlot's default value function in place.
            ...(s.unit != null ? { value: unitValue(s.unit) } : {}),
          }),
        ),
      ],
      axes: [axisTheme, { ...axisTheme, label, size: 56 }],
    }
    const current = new uPlot(opts, data, el)
    plot = current
    // In fill mode the first measurement can predate the legend settling (it
    // has to exist before its height can be read, and its row height depends on
    // the fonts). Correct immediately and again on the next frame — the
    // ResizeObserver below only fires when the host itself changes size, which
    // an in-flow legend row does not do.
    if (props.fill === true) {
      const remeasure = () => {
        // The effect re-runs on a series/theme change and destroys the plot, and
        // the component cleanup does the same on unmount — either way `plot` no
        // longer points at this instance, so don't resize a detached one.
        if (plot !== current) return
        current.setSize({ width: el.clientWidth || 640, height: plotHeight() })
      }
      remeasure()
      requestAnimationFrame(remeasure)
    }
    ro = new ResizeObserver(() => {
      if (plot !== current) return
      current.setSize({ width: el.clientWidth || 640, height: plotHeight() })
    })
    ro.observe(el)
  })

  onCleanup(() => {
    ro?.disconnect()
    plot?.destroy()
    plot = undefined
  })

  return (
    <div class={props.fill === true ? 'h-full min-h-0' : undefined}>
      <Show
        when={hasData()}
        fallback={<p class="py-8 text-center text-sm text-zinc-500 dark:text-zinc-400">No data for this period.</p>}
      >
        <div
          ref={(el) => {
            host = el
          }}
          class={props.fill === true ? 'w-full h-full' : 'w-full'}
        />
      </Show>
    </div>
  )
}
