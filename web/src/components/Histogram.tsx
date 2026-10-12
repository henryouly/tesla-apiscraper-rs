import { For, Show, createMemo } from 'solid-js'

interface Bucket {
  start: number
  count: number
}

/**
 * Grafana histogram equivalent: raw values bucketed client-side into
 * fixed-width buckets (`bucketSize`, `bucketOffset` — matching the
 * drive-stats panel's 10/0), rendered as vertical bars with widths as a %
 * of the max bucket count and axis labels at the bucket starts.
 * Presentational only — pass raw values, bucketing happens here.
 */
export function Histogram(props: {
  values: Array<number>
  bucketSize?: number
  bucketOffset?: number
  unit?: string
}) {
  const size = () => props.bucketSize ?? 10
  const offset = () => props.bucketOffset ?? 0

  const buckets = createMemo((): Array<Bucket> => {
    const s = size()
    const o = offset()
    if (!(s > 0)) return []
    const counts = new Map<number, number>()
    for (const v of props.values) {
      if (!Number.isFinite(v)) continue
      const start = Math.floor((v - o) / s) * s + o
      counts.set(start, (counts.get(start) ?? 0) + 1)
    }
    return [...counts.entries()]
      .sort((a, b) => a[0] - b[0])
      .map(([start, count]) => ({ start, count }))
  })

  const max = () => buckets().reduce((m, b) => Math.max(m, b.count), 0)

  // Thin the x-axis labels so crowded histograms stay readable.
  const labelEvery = () => Math.max(1, Math.ceil(buckets().length / 12))
  const labelFor = (index: number, start: number) => {
    const every = labelEvery()
    if (index % every !== 0 && index !== buckets().length - 1) return ''
    const rounded = Math.round(start * 10) / 10
    return props.unit ? `${rounded} ${props.unit}` : String(rounded)
  }

  return (
    <Show
      when={buckets().length > 0}
      fallback={<p class="py-6 text-center text-sm text-zinc-500 dark:text-zinc-400">No data.</p>}
    >
      <div class="flex h-48 items-end gap-1">
        <For each={buckets()}>
          {(bucket) => (
            <div
              class="flex min-w-0 flex-1 flex-col items-center justify-end gap-1 self-stretch"
              title={`${bucket.start}–${bucket.start + size()}: ${bucket.count}`}
            >
              <p class="shrink-0 text-xs font-semibold tabular-nums text-zinc-800 dark:text-zinc-100">
                {bucket.count}
              </p>
              <div class="flex w-full min-h-0 flex-1 items-end overflow-hidden rounded-t-md bg-zinc-900/[0.06] dark:bg-white/[0.07]">
                <div
                  class="w-full rounded-t-md bg-gradient-to-t from-emerald-600 to-teal-400"
                  style={{ height: `${max() > 0 ? (bucket.count / max()) * 100 : 0}%` }}
                />
              </div>
            </div>
          )}
        </For>
      </div>
      <div class="mt-1 flex gap-1">
        <For each={buckets()}>
          {(bucket, index) => (
            <p class="min-w-0 flex-1 truncate text-center text-[11px] tabular-nums text-zinc-500 dark:text-zinc-400">
              {labelFor(index(), bucket.start)}
            </p>
          )}
        </For>
      </div>
    </Show>
  )
}
