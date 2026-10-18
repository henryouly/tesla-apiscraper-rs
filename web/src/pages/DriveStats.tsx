import { Show, createMemo, createResource } from 'solid-js'
import { useDashboardScope } from '../components/DashboardsLayout'
import { BarGauge } from '../components/BarGauge'
import { BigStat } from '../components/data'
import { Histogram } from '../components/Histogram'
import { Alert, Card, EmptyState, Spinner } from '../components/ui'
import { fetchDriveStats, toEpochMs, type DistanceUnit } from '../lib/dashboards'
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
  // Two decimals: this Grafana panel leaves `decimals` unset on its `kwatth`
  // unit, which falls back to two.
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

  // Days covered by the selected range, the denominator for the per-day stats
  // and the extrapolations below. Elapsed time rather than a calendar-day
  // count, so a "Last 5 minutes" range reads as a fraction of a day instead of
  // being clamped to one.
  const days = createMemo(() => {
    const r = range()
    return (toEpochMs(r.to) - toEpochMs(r.from)) / 86_400_000
  })
  // Both averages divide by every day in the range, not just the days the car
  // moved — a quiet stretch lowers the average and drags the extrapolations
  // down with it, which is the point of reading them as a period rate.
  const perDayDistance = createMemo(() => {
    const d = days()
    return d > 0 ? (driveStats()?.distance ?? 0) / d : null
  })
  const perDayEnergy = createMemo(() => {
    const d = days()
    const e = driveStats()?.energy_kwh
    return d > 0 && e != null ? e / d : null
  })

  const perDayDistanceText = () => {
    const v = perDayDistance()
    return v == null ? '—' : `${round1(v)} ${unit()}`
  }
  // Two decimals, matching the Energy Used panel above.
  const perDayEnergyText = () => {
    const v = perDayEnergy()
    return v == null ? '—' : `${v.toFixed(2)} kWh`
  }
  // Calendar-average days, so the month and the year stay consistent with each
  // other (and with the per-day figure they come from).
  const monthlyMileageText = () => {
    const v = perDayDistance()
    return v == null ? '—' : `${Math.round(v * (365.25 / 12))} ${unit()}`
  }
  const annualMileageText = () => {
    const v = perDayDistance()
    return v == null ? '—' : `${Math.round(v * 365.25)} ${unit()}`
  }

  // The panel is titled "Top 10", so cap it — the backend returns every place
  // in the window (Grafana's bargauge renders them all), and a month of
  // commuting easily has more than ten.
  const topDestinations = createMemo(() => (driveStats()?.top_destinations ?? []).slice(0, 10))

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
            <BigStat label="Ø Distance driven per day" value={perDayDistanceText()} />
            <BigStat label="Ø Energy consumed (net) per day" value={perDayEnergyText()} />
            <BigStat label="Max Speed" value={maxSpeedText()} />
            <BigStat label="Extrapolated monthly mileage" value={monthlyMileageText()} />
            <BigStat label="Extrapolated annual mileage" value={annualMileageText()} />
          </div>
          <Card class="mb-4 p-5">
            <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">
              Speed Distribution
            </h2>
            <Histogram values={driveStats()?.speeds ?? []} unit={`${unit()}/h`} />
          </Card>
          <Card class="mb-4 p-5">
            <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">
              Top 10 destinations (in this period)
            </h2>
            <BarGauge
              items={topDestinations().map((d) => ({
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
