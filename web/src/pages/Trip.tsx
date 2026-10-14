import { Show, createMemo, createResource, onCleanup, onMount } from 'solid-js'
import { useNavigate } from '@solidjs/router'
import L from 'leaflet'
import 'leaflet/dist/leaflet.css'
import { useDashboardScope } from '../components/DashboardsLayout'
import { BigStat, DataTable, type DataColumn } from '../components/data'
import { PieChart } from '../components/PieChart'
import { Alert, Card, EmptyState, Spinner } from '../components/ui'
import { TimeChart, type ChartSeries } from '../lib/charts'
import { fetchTrip, type DistanceUnit, type DriveRow, type RoutePoint, type TripChargeRow } from '../lib/dashboards'
import { useUnits } from '../lib/units'

interface TripDriveTableRow extends Record<string, unknown> {
  t: number
  drive_id: string
  from_t: number
  to_t: number
  distance: number | null
  duration_min: number | null
  from_gf: string | null
  to_gf: string | null
}

interface TripChargeTableRow extends Record<string, unknown> {
  t: number
  charge_id: string
  location: string | null
  energy_kwh: number | null
  cost: number | null
}

function driveToRow(r: DriveRow): TripDriveTableRow {
  return {
    t: r.t,
    drive_id: r.drive_id,
    from_t: r.from_t,
    to_t: r.to_t,
    distance: r.distance,
    duration_min: r.duration_min,
    from_gf: r.from_gf,
    to_gf: r.to_gf,
  }
}

function chargeToRow(r: TripChargeRow): TripChargeTableRow {
  return {
    t: r.t,
    charge_id: r.charge_id,
    location: r.location,
    energy_kwh: r.energy_kwh,
    cost: r.cost,
  }
}

// Humanized duration: seconds below a minute, minutes below an hour,
// "1h 30m" above it. Mirrors the Drives page durationText (seconds in).
function durationText(s: number | null): string {
  if (s == null) return '—'
  if (s < 60) return `${Math.round(s)} s`
  const mins = Math.round(s / 60)
  if (mins < 60) return `${mins} min`
  const h = Math.floor(mins / 60)
  const m = mins % 60
  return m === 0 ? `${h}h` : `${h}h ${m}m`
}

// Route polyline through the window's positions, fitted to bounds. Same
// OSM tile setup as the Drive Details page — deliberately no L.marker
// (its default icon images are bundler-hostile).
function TripMap(props: { route: Array<RoutePoint> }) {
  let mapEl!: HTMLDivElement
  let map: L.Map | undefined
  onMount(() => {
    const latlngs = props.route.map((p) => [p.lat, p.lng]) as Array<[number, number]>
    map = L.map(mapEl)
    L.tileLayer('https://tile.openstreetmap.org/{z}/{x}/{y}.png', {
      maxZoom: 19,
      attribution: '&copy; OpenStreetMap contributors',
    }).addTo(map)
    L.polyline(latlngs, { color: '#1d4ed8', weight: 3 }).addTo(map)
    map.fitBounds(L.latLngBounds(latlngs))
  })
  onCleanup(() => map?.remove())
  return (
    <Card class="mb-4 p-5">
      <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">Trip Map</h2>
      <div class="overflow-hidden rounded-xl ring-1 ring-zinc-900/10 dark:ring-white/10">
        <div ref={(el) => (mapEl = el)} class="h-64 w-full" />
      </div>
    </Card>
  )
}

export function Trip() {
  const units = useUnits()
  const scope = useDashboardScope()
  const range = () => scope.range()
  // Distance unit always follows the global setting (Settings page);
  // there is no page-local override.
  const unit = (): DistanceUnit => (units.unitLength() === 'mi' ? 'mi' : 'km')
  const vin = () => scope.vin()
  const navigate = useNavigate()

  const [trip] = createResource(
    () => {
      const v = vin()
      if (v == null) return false
      return { vin: v, range: range(), unit: unit() }
    },
    (s) => fetchTrip({ vin: s.vin, from: s.range.from, to: s.range.to, unit: s.unit }),
  )

  const driveRows = createMemo(() => (trip()?.drives ?? []).map(driveToRow))
  const chargeRows = createMemo(() => (trip()?.charges ?? []).map(chargeToRow))
  const isEmpty = () =>
    (trip()?.drives.length ?? 0) === 0 &&
    (trip()?.charges.length ?? 0) === 0 &&
    (trip()?.battery.length ?? 0) === 0 &&
    (trip()?.route.length ?? 0) === 0
  const hasRoute = () => (trip()?.route.length ?? 0) > 0

  const round1 = (v: number) => Math.round(v * 10) / 10
  const distanceText = () => `${round1(trip()?.distance ?? 0)} ${unit()}`
  const avgSpeedText = () => {
    const v = trip()?.avg_speed
    return v == null ? '—' : `${round1(v)} ${unit()}/h`
  }
  // Consumption is always Wh/km (no unit factor — same as Efficiency).
  const consumptionText = () => {
    const v = trip()?.consumption_wh_per_km
    return v == null ? '—' : `${Math.round(v)} Wh/km`
  }
  // Cost formatting follows the Charges page conventions: raw numbers
  // from the backend rendered with two decimals (no currency symbol).
  const costText = () => (trip()?.charging_cost ?? 0).toFixed(2)

  const driveColumns = createMemo((): Array<DataColumn<TripDriveTableRow>> => [
    { key: 't', label: 'Time', render: (row) => new Date(row.t).toLocaleString() },
    {
      key: 'distance',
      label: `Distance (${unit()})`,
      align: 'right',
      render: (row) => (row.distance == null ? '—' : String(round1(row.distance))),
    },
    {
      key: 'duration_min',
      label: 'Duration (min)',
      align: 'right',
      render: (row) => (row.duration_min == null ? '—' : String(Math.round(row.duration_min))),
    },
    { key: 'from_gf', label: 'From', render: (row) => row.from_gf ?? '—' },
    { key: 'to_gf', label: 'To', render: (row) => row.to_gf ?? '—' },
  ])

  const chargeColumns = createMemo((): Array<DataColumn<TripChargeTableRow>> => [
    { key: 't', label: 'Time', render: (row) => new Date(row.t).toLocaleString() },
    { key: 'location', label: 'Location', render: (row) => row.location ?? '—' },
    {
      key: 'energy_kwh',
      label: 'Energy (kWh)',
      align: 'right',
      render: (row) => (row.energy_kwh == null ? '—' : row.energy_kwh.toFixed(1)),
    },
    {
      key: 'cost',
      label: 'Cost',
      align: 'right',
      render: (row) => (row.cost == null ? '—' : row.cost.toFixed(2)),
    },
  ])

  const batterySeries = createMemo((): Array<ChartSeries> => [
    {
      label: 'Battery %',
      data: (trip()?.battery ?? []).map((p): [number, number | null] => [p.t, p.level]),
      color: '#10b981',
    },
    {
      label: `Ideal (${unit()})`,
      data: (trip()?.battery ?? []).map((p): [number, number | null] => [p.t, p.ideal]),
      color: '#f59e0b',
    },
  ])

  return (
    <div>
      <Show when={vin()}>
        <Show when={trip.loading && !trip()}>
          <Spinner />
        </Show>
        <Show when={trip.error}>
          <div class="mb-4">
            <Alert tone="error">
              {trip.error instanceof Error ? trip.error.message : 'Failed to load trip data.'}
            </Alert>
          </div>
        </Show>
        <Show when={!trip.loading && !trip.error && trip() && isEmpty()}>
          <EmptyState
            title="No trip data for this period"
            hint="Try a wider time range, or check back once the car completes a drive or charge."
          />
        </Show>
        <Show when={!trip.error && trip() && !isEmpty()}>
          <div class="mb-4 grid gap-4 sm:grid-cols-2">
            <BigStat label={`Distance (${unit()})`} value={distanceText()} />
            <BigStat label={`Avg Speed (${unit()}/h)`} value={avgSpeedText()} />
            <BigStat label="Consumption" value={consumptionText()} />
            <BigStat label="Charging Cost" value={costText()} />
          </div>
          <Card class="mb-4 p-5">
            <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">Time Spent</h2>
            <PieChart
              slices={[
                { label: 'Driving', value: trip()?.driving_s ?? 0, color: '#3b82f6' },
                { label: 'Charging', value: trip()?.charging_s ?? 0, color: '#10b981' },
              ]}
            />
            <p class="mt-3 text-center text-xs tabular-nums text-zinc-500 dark:text-zinc-400">
              {`Driving ${durationText(trip()?.driving_s ?? 0)} · Charging ${durationText(trip()?.charging_s ?? 0)}`}
            </p>
          </Card>
          <Show when={hasRoute()}>
            <TripMap route={trip()!.route} />
          </Show>
          <Card class="mb-4 p-5">
            <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">Drives</h2>
            <DataTable
              columns={driveColumns()}
              rows={driveRows()}
              onRowClick={(row) => {
                if (!row.drive_id) return
                // Same drill-down as the Drives page: the detail page
                // resolves its own window, but the row's from/to travels
                // with the navigation.
                const from = new Date(row.from_t).toISOString()
                const to = new Date(row.to_t).toISOString()
                navigate(
                  `/dashboards/drives/${encodeURIComponent(row.drive_id)}?from=${encodeURIComponent(from)}&to=${encodeURIComponent(to)}`,
                )
              }}
            />
          </Card>
          <Card class="mb-4 p-5">
            <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">Charges</h2>
            <DataTable
              columns={chargeColumns()}
              rows={chargeRows()}
              onRowClick={(row) => {
                if (!row.charge_id) return
                navigate(`/dashboards/charging-stats?charge_id=${encodeURIComponent(row.charge_id)}`)
              }}
            />
          </Card>
          <Card class="mb-4 p-5">
            <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">Battery Level &amp; Range</h2>
            <TimeChart series={batterySeries()} height={260} />
          </Card>
        </Show>
      </Show>
    </div>
  )
}
