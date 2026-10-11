import { Show, createMemo, createResource, createSignal } from 'solid-js'
import { useDashboardScope } from '../components/DashboardsLayout'
import { Alert, Card, EmptyState, Spinner } from '../components/ui'
import { TimeChart, type ChartSeries } from '../lib/charts'
import { defaultRange, fetchOverview, type DistanceUnit, type Range } from '../lib/dashboards'
import { useUnits } from '../lib/units'

export function Overview() {
  const units = useUnits()
  const scope = useDashboardScope()
  // Fixed 7-day window for now; a range picker lands in a later stage.
  const [range] = createSignal<Range>(defaultRange(7))
  // Distance unit always follows the global setting (Settings page);
  // there is no page-local override.
  const unit = (): DistanceUnit => (units.unitLength() === 'mi' ? 'mi' : 'km')
  const vin = () => scope.vin()

  const [overview] = createResource(
    () => {
      const v = vin()
      if (v == null) return false
      return { vin: v, range: range(), unit: unit() }
    },
    (s) => fetchOverview({ vin: s.vin, from: s.range.from, to: s.range.to, unit: s.unit }),
  )

  const isEmpty = () => (overview()?.battery.length ?? 0) === 0 && (overview()?.ideal_range.length ?? 0) === 0

  const chargeSeries = createMemo((): Array<ChartSeries> => {
    const battery = overview()?.battery ?? []
    return [
      {
        label: 'Battery Level',
        data: battery.map((p): [number, number | null] => [p.t, p.level]),
        color: '#10b981',
      },
      {
        label: 'Usable',
        data: battery.map((p): [number, number | null] => [p.t, p.usable]),
        color: '#0ea5e9',
      },
    ]
  })

  const idealSeries = createMemo((): Array<ChartSeries> => [
    {
      label: 'Ideal Range',
      data: (overview()?.ideal_range ?? []).map((p): [number, number | null] => [p.t, p.value]),
      color: '#e82127',
    },
  ])

  return (
    <div>
      <Show when={vin()}>
        <Show when={overview.loading && !overview()}>
          <Spinner />
        </Show>
        <Show when={overview.error}>
          <div class="mb-4">
            <Alert tone="error">
              {overview.error instanceof Error ? overview.error.message : 'Failed to load overview data.'}
            </Alert>
          </div>
        </Show>
        <Show when={!overview.loading && !overview.error && overview() && isEmpty()}>
          <EmptyState
            title="No data for this period"
            hint="Try a different vehicle, or check back once the car reports telemetry."
          />
        </Show>
        <Show when={!overview.error && overview() && !isEmpty()}>
          <Card class="mb-4 p-5">
            <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">Charge Level</h2>
            <TimeChart series={chargeSeries()} height={260} yUnit="%" />
          </Card>
          <Card class="mb-4 p-5">
            <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">Ideal Range</h2>
            <TimeChart series={idealSeries()} height={260} yUnit={unit()} />
          </Card>
        </Show>
      </Show>
    </div>
  )
}
