import { Show, createMemo, createResource } from 'solid-js'
import { useDashboardScope } from '../components/DashboardsLayout'
import { Alert, Card, EmptyState, Spinner } from '../components/ui'
import { TimeChart, type ChartSeries } from '../lib/charts'
import { fetchProjectedRange, type DistanceUnit } from '../lib/dashboards'
import { useUnits } from '../lib/units'

export function ProjectedRange() {
  const units = useUnits()
  const scope = useDashboardScope()
  const range = () => scope.range()
  // Distance unit always follows the global setting (Settings page);
  // there is no page-local override.
  const unit = (): DistanceUnit => (units.unitLength() === 'mi' ? 'mi' : 'km')
  const vin = () => scope.vin()

  const [projectedRange] = createResource(
    () => {
      const v = vin()
      if (v == null) return false
      return { vin: v, range: range(), unit: unit() }
    },
    (s) => fetchProjectedRange({ vin: s.vin, from: s.range.from, to: s.range.to, unit: s.unit }),
  )

  const isEmpty = () =>
    (projectedRange()?.projected.length ?? 0) === 0 &&
    (projectedRange()?.outside_temp.length ?? 0) === 0

  const projectedSeries = createMemo((): Array<ChartSeries> => {
    const projected = projectedRange()?.projected ?? []
    return [
      {
        label: 'Projected Ideal',
        data: projected.map((p): [number, number | null] => [p.t, p.ideal]),
        color: '#10b981',
      },
      {
        label: 'Projected Rated',
        data: projected.map((p): [number, number | null] => [p.t, p.rated]),
        color: '#0ea5e9',
      },
    ]
  })

  const tempSeries = createMemo((): Array<ChartSeries> => [
    {
      label: 'Outside',
      data: (projectedRange()?.outside_temp ?? []).map((p): [number, number | null] => [p.t, p.value]),
      color: '#e82127',
    },
  ])

  return (
    <div>
      <Show when={vin()}>
        <Show when={projectedRange.loading && !projectedRange()}>
          <Spinner />
        </Show>
        <Show when={projectedRange.error}>
          <div class="mb-4">
            <Alert tone="error">
              {projectedRange.error instanceof Error
                ? projectedRange.error.message
                : 'Failed to load projected range data.'}
            </Alert>
          </div>
        </Show>
        <Show when={!projectedRange.loading && !projectedRange.error && projectedRange() && isEmpty()}>
          <EmptyState
            title="No data for this period"
            hint="Try a different vehicle, or check back once the car reports telemetry."
          />
        </Show>
        <Show when={!projectedRange.error && projectedRange() && !isEmpty()}>
          <Card class="mb-4 p-5">
            <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">
              Projected Range at 100%
            </h2>
            <TimeChart series={projectedSeries()} height={260} yUnit={unit()} />
          </Card>
          <Card class="mb-4 p-5">
            <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">
              Outside Temperature
            </h2>
            <TimeChart series={tempSeries()} height={260} yUnit="°C" />
          </Card>
        </Show>
      </Show>
    </div>
  )
}
