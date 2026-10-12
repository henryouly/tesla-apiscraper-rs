import { Show, createMemo, createResource } from 'solid-js'
import { useNavigate } from '@solidjs/router'
import { useDashboardScope } from '../components/DashboardsLayout'
import { BigStat, DataTable, type DataColumn } from '../components/data'
import { Alert, Card, EmptyState, Spinner } from '../components/ui'
import { TimeChart, type ChartSeries } from '../lib/charts'
import { fetchCharges, type ChargeSessionRow } from '../lib/dashboards'

interface ChargeTableRow extends Record<string, unknown> {
  t: number
  location: string | null
  energy_kwh: number | null
  duration_min: number | null
  cost: number | null
  start_pct: number | null
  end_pct: number | null
}

function toRow(r: ChargeSessionRow): ChargeTableRow {
  return {
    t: r.t,
    location: r.location,
    energy_kwh: r.energy_kwh,
    duration_min: r.duration_min,
    cost: r.cost,
    start_pct: r.start_pct,
    end_pct: r.end_pct,
  }
}

// Cost formatting follows the ChargeCost page conventions: raw numbers from
// the backend rendered with two decimals (no currency symbol — the backend
// returns plain cost figures and there is no currency setting).
function costText(v: number | null): string {
  return v == null ? '—' : v.toFixed(2)
}

export function Charges() {
  const scope = useDashboardScope()
  const range = () => scope.range()
  const vin = () => scope.vin()
  const navigate = useNavigate()

  const [charges] = createResource(
    () => {
      const v = vin()
      if (v == null) return false
      return { vin: v, range: range() }
    },
    (s) => fetchCharges({ vin: s.vin, from: s.range.from, to: s.range.to }),
  )

  const rows = createMemo(() => (charges()?.sessions ?? []).map(toRow))
  const isEmpty = () =>
    (charges()?.count ?? 0) === 0 &&
    (charges()?.sessions.length ?? 0) === 0 &&
    (charges()?.energy_per_day.length ?? 0) === 0

  const energyText = () => `${(charges()?.energy_kwh ?? 0).toFixed(1)} kWh`
  const totalCostText = () => costText(charges()?.total_cost ?? 0)
  const avgCostText = () => {
    const v = charges()?.avg_cost_per_kwh
    return v == null ? '—' : v.toFixed(2)
  }

  const columns = createMemo((): Array<DataColumn<ChargeTableRow>> => [
    { key: 't', label: 'Time', render: (row) => new Date(row.t).toLocaleString() },
    { key: 'location', label: 'Location' },
    {
      key: 'energy_kwh',
      label: 'Energy (kWh)',
      align: 'right',
      render: (row) => (row.energy_kwh == null ? '—' : row.energy_kwh.toFixed(1)),
    },
    {
      key: 'duration_min',
      label: 'Duration (min)',
      align: 'right',
      render: (row) => (row.duration_min == null ? '—' : String(Math.round(row.duration_min))),
    },
    {
      key: 'cost',
      label: 'Cost',
      align: 'right',
      render: (row) => costText(row.cost),
    },
    {
      key: 'start_pct',
      label: 'Start %',
      align: 'right',
      render: (row) => (row.start_pct == null ? '—' : String(Math.round(row.start_pct))),
    },
    {
      key: 'end_pct',
      label: 'End %',
      align: 'right',
      render: (row) => (row.end_pct == null ? '—' : String(Math.round(row.end_pct))),
    },
  ])

  // Grafana renders Energy per Day as bars; TimeChart is line-only, so this
  // diverges visually (zeros still show correctly as zero points).
  const energySeries = createMemo((): Array<ChartSeries> => [
    {
      label: 'Energy',
      data: (charges()?.energy_per_day ?? []).map((p): [number, number | null] => [p.t, p.value]),
      color: '#10b981',
    },
  ])

  const powerSeries = createMemo((): Array<ChartSeries> =>
    (charges()?.power_by_session ?? []).map((s) => ({
      label: s.charge_id,
      data: s.points.map((p): [number, number | null] => [p.t, p.value]),
    })),
  )

  return (
    <div>
      <Show when={vin()}>
        <Show when={charges.loading && !charges()}>
          <Spinner />
        </Show>
        <Show when={charges.error}>
          <div class="mb-4">
            <Alert tone="error">
              {charges.error instanceof Error ? charges.error.message : 'Failed to load charges data.'}
            </Alert>
          </div>
        </Show>
        <Show when={!charges.loading && !charges.error && charges() && isEmpty()}>
          <EmptyState
            title="No charges for this period"
            hint="Try a wider time range, or check back once the car completes a charge."
          />
        </Show>
        <Show when={!charges.error && charges() && !isEmpty()}>
          <div class="mb-4 grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
            <BigStat label="# of Charges" value={String(charges()?.count ?? 0)} />
            <BigStat label="Energy Added" value={energyText()} />
            <BigStat label="Total Cost" value={totalCostText()} />
            <BigStat label="Avg Cost / kWh" value={avgCostText()} />
          </div>
          <Card class="mb-4 p-5">
            <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">Charges</h2>
            <DataTable
              columns={columns()}
              rows={rows()}
              onRowClick={() => navigate('/dashboards/charging-stats')}
            />
          </Card>
          <Card class="mb-4 p-5">
            <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">Energy per Day</h2>
            <TimeChart series={energySeries()} height={260} yUnit="kWh" />
          </Card>
          <Card class="mb-4 p-5">
            <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">
              Charging Power by Session
            </h2>
            <TimeChart series={powerSeries()} height={260} yUnit="kW" />
          </Card>
        </Show>
      </Show>
    </div>
  )
}
