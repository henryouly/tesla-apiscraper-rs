import { Show, createMemo, createResource } from 'solid-js'
import { useDashboardScope } from '../components/DashboardsLayout'
import { Alert, Card, EmptyState, Spinner } from '../components/ui'
import { TimeChart, type ChartSeries } from '../lib/charts'
import { fetchMileage, type DistanceUnit } from '../lib/dashboards'
import { useUnits } from '../lib/units'

export function Mileage() {
  const units = useUnits()
  const scope = useDashboardScope()
  const range = () => scope.range()
  // Distance unit always follows the global setting (Settings page);
  // there is no page-local override.
  const unit = (): DistanceUnit => (units.unitLength() === 'mi' ? 'mi' : 'km')
  const vin = () => scope.vin()

  const [mileage] = createResource(
    () => {
      const v = vin()
      if (v == null) return false
      return { vin: v, range: range(), unit: unit() }
    },
    (s) => fetchMileage({ vin: s.vin, from: s.range.from, to: s.range.to, unit: s.unit }),
  )

  const isEmpty = () => (mileage()?.points.length ?? 0) === 0

  const series = createMemo((): Array<ChartSeries> => [
    {
      label: 'Mileage',
      data: (mileage()?.points ?? []).map((p): [number, number | null] => [p.t, p.value]),
      color: '#0ea5e9',
    },
  ])

  return (
    <div>
      <Show when={vin()}>
        <Show when={mileage.loading && !mileage()}>
          <Spinner />
        </Show>
        <Show when={mileage.error}>
          <div class="mb-4">
            <Alert tone="error">
              {mileage.error instanceof Error ? mileage.error.message : 'Failed to load mileage data.'}
            </Alert>
          </div>
        </Show>
        <Show when={!mileage.loading && !mileage.error && mileage() && isEmpty()}>
          <EmptyState
            title="No data for this period"
            hint="Try a different vehicle, or check back once the car reports telemetry."
          />
        </Show>
        <Show when={!mileage.error && mileage() && !isEmpty()}>
          <Card class="mb-4 p-5">
            <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">Mileage</h2>
            <TimeChart series={series()} height={260} yUnit={unit()} />
          </Card>
        </Show>
      </Show>
    </div>
  )
}
