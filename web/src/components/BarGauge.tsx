import { For, Show } from 'solid-js'

export interface BarGaugeItem {
  label: string
  value: number
}

/**
 * Grafana bargauge equivalent: horizontal bars sorted desc, each bar width
 * a % of the max value. Presentational only — pass pre-sorted or unsorted
 * items, sorting happens here.
 */
export function BarGauge(props: { items: Array<BarGaugeItem>; unit?: string }) {
  const sorted = () => [...props.items].sort((a, b) => b.value - a.value)
  const max = () => sorted().reduce((m, i) => Math.max(m, i.value), 0)

  return (
    <Show
      when={sorted().length > 0}
      fallback={<p class="py-6 text-center text-sm text-zinc-500 dark:text-zinc-400">No data.</p>}
    >
      <div class="flex flex-col gap-2.5">
        <For each={sorted()}>
          {(item) => (
            <div class="flex items-center gap-3">
              <p class="w-36 shrink-0 truncate text-sm text-zinc-700 dark:text-zinc-200" title={item.label}>
                {item.label}
              </p>
              <div class="h-5 min-w-0 flex-1 overflow-hidden rounded-md bg-zinc-900/[0.06] dark:bg-white/[0.07]">
                <div
                  class="h-full rounded-md bg-gradient-to-r from-emerald-500 to-teal-400"
                  style={{ width: `${max() > 0 ? (item.value / max()) * 100 : 0}%` }}
                />
              </div>
              <p class="w-20 shrink-0 text-right text-sm font-semibold tabular-nums text-zinc-800 dark:text-zinc-100">
                {item.value}
                <Show when={props.unit}>
                  <span class="ml-1 font-normal text-zinc-500 dark:text-zinc-400">{props.unit}</span>
                </Show>
              </p>
            </div>
          )}
        </For>
      </div>
    </Show>
  )
}
