import { Show, createMemo, createResource } from 'solid-js'
import { useDashboardScope } from '../components/DashboardsLayout'
import { BigStat } from '../components/data'
import { Alert, Card, EmptyState, Spinner } from '../components/ui'
import { TimeChart, type ChartSeries } from '../lib/charts'
import { fetchEfficiency, type DistanceUnit } from '../lib/dashboards'
import { useUnits } from '../lib/units'

export function Efficiency() {
  const units = useUnits()
  const scope = useDashboardScope()
  const range = () => scope.range()
  // Distance unit always follows the global setting (Settings page);
  // there is no page-local override.
  const unit = (): DistanceUnit => (units.unitLength() === 'mi' ? 'mi' : 'km')
  const vin = () => scope.vin()

  const [efficiency] = createResource(
    () => {
      const v = vin()
      if (v == null) return false
      return { vin: v, range: range(), unit: unit() }
    },
    (s) => fetchEfficiency({ vin: s.vin, from: s.range.from, to: s.range.to, unit: s.unit }),
  )

  const isEmpty = () => (efficiency()?.per_drive.length ?? 0) === 0

  // Avg consumption is always Wh/km (the backend applies no unit factor).
  // One decimal matches the Grafana stat panel; the extra digit the fractional
  // placeholder values needed is gone now that energy is in real watt-hours.
  const avgText = () => {
    const v = efficiency()?.avg_consumption_wh_per_km
    return v == null ? '—' : `${v.toFixed(1)} Wh/km`
  }

  const distanceText = () => `${Math.round((efficiency()?.logged_distance ?? 0) * 10) / 10} ${unit()}`

  const series = createMemo((): Array<ChartSeries> => [
    {
      label: 'Wh/km',
      data: (efficiency()?.per_drive ?? []).map((p): [number, number | null] => [p.t, p.value]),
      color: '#10b981',
    },
  ])

  return (
    <div>
      <Show when={vin()}>
        <Show when={efficiency.loading && !efficiency()}>
          <Spinner />
        </Show>
        <Show when={efficiency.error}>
          <div class="mb-4">
            <Alert tone="error">
              {efficiency.error instanceof Error
                ? efficiency.error.message
                : 'Failed to load efficiency data.'}
            </Alert>
          </div>
        </Show>
        <Show when={!efficiency.loading && !efficiency.error && efficiency() && isEmpty()}>
          <EmptyState
            title="No data for this period"
            hint="Try a different vehicle, or check back once the car reports telemetry."
          />
        </Show>
        <Show when={!efficiency.error && efficiency() && !isEmpty()}>
          <div class="mb-4 grid gap-4 sm:grid-cols-2">
            <BigStat label="Avg Consumption" value={avgText()} sub="gross" />
            <BigStat label="Logged Distance" value={distanceText()} />
          </div>
          <Card class="mb-4 p-5">
            <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">
              Consumption per Drive
            </h2>
            <TimeChart series={series()} height={260} yUnit="Wh/km" />
          </Card>
        </Show>
      </Show>
    </div>
  )
}
