import { Show, createMemo, createResource } from 'solid-js'
import { A } from '@solidjs/router'
import { useDashboardScope } from '../components/DashboardsLayout'
import { BigStat, DataTable, type DataColumn } from '../components/data'
import { Alert, Card, EmptyState, Spinner } from '../components/ui'
import { TimeChart, type ChartSeries } from '../lib/charts'
import { fetchChargingStats, type TopStation } from '../lib/dashboards'

interface StationTableRow extends Record<string, unknown> {
  station: string
  energy_kwh: number
  sessions: number
}

function toRow(r: TopStation): StationTableRow {
  return { station: r.station, energy_kwh: r.energy_kwh, sessions: r.sessions }
}

// Cost formatting follows the Charges page conventions: raw numbers from the
// backend rendered with two decimals (no currency symbol).
function costText(v: number | null): string {
  return v == null ? '—' : v.toFixed(2)
}

export function ChargingStats() {
  const scope = useDashboardScope()
  const range = () => scope.range()
  const vin = () => scope.vin()

  const [stats] = createResource(
    () => {
      const v = vin()
      if (v == null) return false
      return { vin: v, range: range() }
    },
    (s) => fetchChargingStats({ vin: s.vin, from: s.range.from, to: s.range.to }),
  )

  const rows = createMemo(() => (stats()?.top_stations ?? []).map(toRow))
  const isEmpty = () =>
    (stats()?.count ?? 0) === 0 &&
    (stats()?.soc_delta.length ?? 0) === 0 &&
    (stats()?.top_stations.length ?? 0) === 0

  const energyText = () => `${(stats()?.energy_kwh ?? 0).toFixed(1)} kWh`
  const totalCostText = () => costText(stats()?.total_cost ?? 0)
  const avgCostText = () => {
    const v = stats()?.avg_cost_per_kwh
    return v == null ? '—' : v.toFixed(2)
  }

  const columns = createMemo((): Array<DataColumn<StationTableRow>> => [
    { key: 'station', label: 'Station' },
    {
      key: 'energy_kwh',
      label: 'Energy (kWh)',
      align: 'right',
      render: (row) => row.energy_kwh.toFixed(1),
    },
    {
      key: 'sessions',
      label: 'Sessions',
      align: 'right',
      render: (row) => String(row.sessions),
    },
  ])

  const socSeries = createMemo((): Array<ChartSeries> => [
    {
      label: 'Start SOC',
      data: (stats()?.soc_delta ?? []).map((p): [number, number | null] => [p.t, p.start]),
    },
    {
      label: 'End SOC',
      data: (stats()?.soc_delta ?? []).map((p): [number, number | null] => [p.t, p.end]),
    },
  ])

  return (
    <div>
      <A
        href="/dashboards/charges"
        class="mb-4 inline-block text-sm font-medium text-zinc-500 hover:text-zinc-900 dark:text-zinc-400 dark:hover:text-zinc-100"
      >
        ← Back to charges
      </A>
      <Show when={vin()}>
        <Show when={stats.loading && !stats()}>
          <Spinner />
        </Show>
        <Show when={stats.error}>
          <div class="mb-4">
            <Alert tone="error">
              {stats.error instanceof Error ? stats.error.message : 'Failed to load charging stats data.'}
            </Alert>
          </div>
        </Show>
        <Show when={!stats.loading && !stats.error && stats() && isEmpty()}>
          <EmptyState
            title="No charges for this period"
            hint="Try a wider time range, or check back once the car completes a charge."
          />
        </Show>
        <Show when={!stats.error && stats() && !isEmpty()}>
          <div class="mb-4 grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
            <BigStat label="# of Charges" value={String(stats()?.count ?? 0)} />
            <BigStat label="Energy Added" value={energyText()} />
            <BigStat label="Total Cost" value={totalCostText()} />
            <BigStat label="Avg Cost / kWh" value={avgCostText()} />
          </div>
          <Card class="mb-4 p-5">
            <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">Charge Level Delta</h2>
            <TimeChart series={socSeries()} height={260} yUnit="%" />
          </Card>
          <Card class="mb-4 p-5">
            <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">Top Charging Stations</h2>
            <DataTable columns={columns()} rows={rows()} />
          </Card>
        </Show>
      </Show>
    </div>
  )
}
