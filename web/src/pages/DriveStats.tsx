import { Show, createResource } from 'solid-js'
import { useDashboardScope } from '../components/DashboardsLayout'
import { BarGauge } from '../components/BarGauge'
import { BigStat } from '../components/data'
import { Histogram } from '../components/Histogram'
import { Alert, Card, EmptyState, Spinner } from '../components/ui'
import { fetchDriveStats, type DistanceUnit } from '../lib/dashboards'
import { useUnits } from '../lib/units'

export function DriveStats() {
  const units = useUnits()
  const scope = useDashboardScope()
  const range = () => scope.range()
  // Distance unit always follows the global setting (Settings page);
  // there is no page-local override.
  const unit = (): DistanceUnit => (units.unitLength() === 'mi' ? 'mi' : 'km')
  const vin = () => scope.vin()

  const [driveStats] = createResource(
    () => {
      const v = vin()
      if (v == null) return false
      return { vin: v, range: range(), unit: unit() }
    },
    (s) => fetchDriveStats({ vin: s.vin, from: s.range.from, to: s.range.to, unit: s.unit }),
  )

  const isEmpty = () =>
    (driveStats()?.drives ?? 0) === 0 &&
    (driveStats()?.speeds.length ?? 0) === 0 &&
    (driveStats()?.top_destinations.length ?? 0) === 0

  const round1 = (v: number) => Math.round(v * 10) / 10
  const distanceText = () => `${round1(driveStats()?.distance ?? 0)} ${unit()}`
  // Energy figures are 1000x under until #93 lands, so values are currently
  // fractional — two decimals like the efficiency average.
  const energyText = () => {
    const v = driveStats()?.energy_kwh
    return v == null ? '—' : `${v.toFixed(2)} kWh`
  }
  const medianText = () => {
    const v = driveStats()?.median_drive
    return v == null ? '—' : `${round1(v)} ${unit()}`
  }
  const maxSpeedText = () => {
    const v = driveStats()?.max_speed
    return v == null ? '—' : `${round1(v)} ${unit()}/h`
  }

  return (
    <div>
      <Show when={vin()}>
        <Show when={driveStats.loading && !driveStats()}>
          <Spinner />
        </Show>
        <Show when={driveStats.error}>
          <div class="mb-4">
            <Alert tone="error">
              {driveStats.error instanceof Error
                ? driveStats.error.message
                : 'Failed to load drive stats data.'}
            </Alert>
          </div>
        </Show>
        <Show when={!driveStats.loading && !driveStats.error && driveStats() && isEmpty()}>
          <EmptyState
            title="No drives for this period"
            hint="Try a wider time range, or check back once the car completes a drive."
          />
        </Show>
        <Show when={!driveStats.error && driveStats() && !isEmpty()}>
          <div class="mb-4 grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
            <BigStat label="# of Drives" value={String(driveStats()?.drives ?? 0)} />
            <BigStat label="Total Distance" value={distanceText()} />
            <BigStat label="Energy Used" value={energyText()} />
            <BigStat label="Median Drive" value={medianText()} />
            <BigStat label="Max Speed" value={maxSpeedText()} />
          </div>
          <Card class="mb-4 p-5">
            <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">
              Speed Distribution
            </h2>
            <Histogram values={driveStats()?.speeds ?? []} unit={`${unit()}/h`} />
          </Card>
          <Card class="mb-4 p-5">
            <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">
              Top Destinations
            </h2>
            <BarGauge
              items={(driveStats()?.top_destinations ?? []).map((d) => ({
                label: d.place,
                value: d.visits,
              }))}
              unit="visits"
            />
          </Card>
        </Show>
      </Show>
    </div>
  )
}
