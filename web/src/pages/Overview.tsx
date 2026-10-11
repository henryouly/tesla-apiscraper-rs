import { Show, createEffect, createMemo, createResource, createSignal } from 'solid-js'
import { DashboardsNav } from '../components/DashboardsNav'
import { Alert, Card, EmptyState, PageHeader, Select, Spinner } from '../components/ui'
import { api } from '../lib/api'
import { TimeChart, type ChartSeries } from '../lib/charts'
import { defaultRange, fetchOverview, type DistanceUnit, type Range } from '../lib/dashboards'
import { useUnits } from '../lib/units'

export function Overview() {
  const units = useUnits()
  // Fixed 7-day window for now; a range picker lands in a later stage.
  const [range] = createSignal<Range>(defaultRange(7))
  // Distance unit always follows the global setting (Settings page);
  // there is no page-local override.
  const unit = (): DistanceUnit => (units.unitLength() === 'mi' ? 'mi' : 'km')
  const [vin, setVin] = createSignal<string | null>(null)

  const [vehicles] = createResource(() => api.vehicles())

  // Default to the first known vehicle once discovery resolves.
  createEffect(() => {
    if (vin() == null) {
      const first = vehicles()?.vehicles[0]
      if (first) setVin(first.vin)
    }
  })

  const [overview] = createResource(
    () => {
      const v = vin()
      if (v == null) return false
      return { vin: v, range: range(), unit: unit() }
    },
    (s) => fetchOverview({ vin: s.vin, from: s.range.from, to: s.range.to, unit: s.unit }),
  )

  const vinOptions = () => vehicles()?.vehicles.map((v) => ({ value: v.vin, label: v.display_name || v.vin })) ?? []

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
    <div class="flex flex-col gap-6 md:flex-row">
      <DashboardsNav />
      <div class="min-w-0 flex-1">
      <PageHeader
        eyebrow="Dashboards"
        title="Overview"
        hint="Charge level and ideal range for the last 7 days."
      />
      <Show when={vehicles.loading}>
        <Spinner />
      </Show>
      <Show when={vehicles.error}>
        <Alert tone="error">
          {vehicles.error instanceof Error ? vehicles.error.message : 'Failed to load vehicles.'}
        </Alert>
      </Show>
      <Show when={!vehicles.loading && !vehicles.error && (vehicles()?.vehicles.length ?? 0) === 0}>
        <EmptyState
          title="No vehicles found"
          hint="Once a car is linked to this Tesla account and reports telemetry, it will appear here."
        />
      </Show>
      <Show when={!vehicles.loading && !vehicles.error && (vehicles()?.vehicles.length ?? 0) > 0}>
        <Card class="mb-4 p-5">
          <div class="grid gap-4 sm:grid-cols-2 sm:items-end">
            <Select label="Vehicle" value={vin() ?? ''} onChange={setVin} options={vinOptions()} />
          </div>
        </Card>
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
      </Show>
      </div>
    </div>
  )
}
