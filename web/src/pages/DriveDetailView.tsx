import { Show, createMemo, createResource, onCleanup, onMount } from 'solid-js'
import { A, useParams } from '@solidjs/router'
import L from 'leaflet'
import 'leaflet/dist/leaflet.css'
import { BigStat } from '../components/data'
import { Alert, Card, EmptyState, Spinner } from '../components/ui'
import { TimeChart, type ChartSeries } from '../lib/charts'
import { ApiError } from '../lib/api'
import { fetchDriveDetails, type DistanceUnit, type RoutePoint } from '../lib/dashboards'
import { useUnits } from '../lib/units'

// Shared crosshair across the Drive / Elevation / Temperatures charts
// (matches Grafana `graphTooltip: 1`).
const SYNC_KEY = 'drive-details'

// Humanized drive length: minutes below an hour, "1h 30m" above it.
// Mirrors the Drives page durationText.
function durationText(s: number | null): string {
  if (s == null) return '—'
  const mins = Math.round(s / 60)
  if (mins < 60) return `${mins} min`
  const h = Math.floor(mins / 60)
  const m = mins % 60
  return m === 0 ? `${h}h` : `${h}h ${m}m`
}

// Route polyline through the drive's positions, fitted to bounds. Same OSM
// tile setup as the Geofences page — deliberately no L.marker (its default
// icon images are bundler-hostile).
function RouteMap(props: { route: Array<RoutePoint> }) {
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
      <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">Route</h2>
      <div class="overflow-hidden rounded-xl ring-1 ring-zinc-900/10 dark:ring-white/10">
        <div ref={(el) => (mapEl = el)} class="h-64 w-full" />
      </div>
    </Card>
  )
}

/** Per-drive detail drill-down (route `/dashboards/drives/:driveId`). */
export function DriveDetailView() {
  // The server resolves the drive window from the drives table itself, so
  // the ?from&to query params the Drives row navigation appends are
  // ignored here (client fallback only if ever needed — never sent).
  const params = useParams()
  const driveId = () => params.driveId
  const units = useUnits()
  // Distance unit always follows the global setting (Settings page);
  // there is no page-local override.
  const unit = (): DistanceUnit => (units.unitLength() === 'mi' ? 'mi' : 'km')

  const [detail] = createResource(
    () => {
      const id = driveId()
      if (id == null || id.length === 0) return false
      return { id, unit: unit() }
    },
    (s) => fetchDriveDetails(s.id, s.unit),
  )
  const isNotFound = () => detail.error instanceof ApiError && detail.error.status === 404
  const hasRoute = () => (detail()?.route.length ?? 0) > 0

  const round1 = (v: number) => Math.round(v * 10) / 10
  const distanceText = () => {
    const v = detail()?.stats.distance
    return v == null ? '—' : `${round1(v)} ${unit()}`
  }
  // One decimal per the Grafana panel (decimals: 1).
  const energyText = () => {
    const v = detail()?.stats.energy_kwh
    return v == null ? '—' : `${v.toFixed(1)} kWh`
  }
  const avgSpeedText = () => {
    const v = detail()?.stats.avg_speed
    return v == null ? '—' : `${round1(v)} ${unit()}/h`
  }
  const odometerText = () => {
    const o = detail()?.odometer
    if (o?.from == null || o?.to == null) return '—'
    return `${round1(o.from)} → ${round1(o.to)} ${unit()}`
  }

  // The Drive chart mixes units (speed, kW, %, range), so it carries no
  // single y-axis unit — same as the Grafana panel's default short unit.
  const driveSeries = createMemo((): Array<ChartSeries> => [
    {
      label: 'Speed',
      data: (detail()?.series ?? []).map((p): [number, number | null] => [p.t, p.speed]),
      color: '#e82127',
    },
    {
      label: 'Power kW',
      data: (detail()?.series ?? []).map((p): [number, number | null] => [p.t, p.power_kw]),
      color: '#0ea5e9',
    },
    {
      label: 'Battery %',
      data: (detail()?.series ?? []).map((p): [number, number | null] => [p.t, p.battery]),
      color: '#10b981',
    },
    {
      label: 'Ideal',
      data: (detail()?.series ?? []).map((p): [number, number | null] => [p.t, p.ideal]),
      color: '#f59e0b',
    },
  ])

  const elevationSeries = createMemo((): Array<ChartSeries> => [
    {
      label: 'Elevation',
      data: (detail()?.elevation ?? []).map((p): [number, number | null] => [p.t, p.value]),
      color: '#8b5cf6',
    },
  ])

  const tempSeries = createMemo((): Array<ChartSeries> => [
    {
      label: 'Outside',
      data: (detail()?.temps ?? []).map((p): [number, number | null] => [p.t, p.outside]),
      color: '#0ea5e9',
    },
    {
      label: 'Inside',
      data: (detail()?.temps ?? []).map((p): [number, number | null] => [p.t, p.inside]),
      color: '#e82127',
    },
  ])

  return (
    <div>
      <A
        href="/dashboards/drives"
        class="mb-4 inline-block text-sm font-medium text-zinc-500 hover:text-zinc-900 dark:text-zinc-400 dark:hover:text-zinc-100"
      >
        ← Back to drives
      </A>
      <Show when={detail.loading && !detail()}>
        <Spinner />
      </Show>
      <Show when={isNotFound()}>
        <EmptyState
          title="Drive not found"
          hint="This drive no longer exists, or the link is stale."
        />
      </Show>
      <Show when={!isNotFound() && detail.error}>
        <div class="mb-4">
          <Alert tone="error">
            {detail.error instanceof Error ? detail.error.message : 'Failed to load drive details.'}
          </Alert>
        </div>
      </Show>
      <Show when={!detail.error && detail()}>
        <div class="mb-4 grid gap-4 sm:grid-cols-2">
          <BigStat label="Duration" value={durationText(detail()?.stats.duration_s ?? null)} />
          <BigStat label={`Distance (${unit()})`} value={distanceText()} />
          <BigStat label="Energy Used" value={energyText()} />
          <BigStat label={`Avg Speed (${unit()}/h)`} value={avgSpeedText()} />
        </div>
        <Card class="mb-4 p-5">
          <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">Drive</h2>
          <TimeChart series={driveSeries()} height={260} syncKey={SYNC_KEY} />
        </Card>
        <Show when={hasRoute()}>
          <RouteMap route={detail()!.route} />
        </Show>
        <Card class="mb-4 p-5">
          <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">Elevation</h2>
          <TimeChart series={elevationSeries()} height={260} yUnit="m" syncKey={SYNC_KEY} />
        </Card>
        <Card class="mb-4 p-5">
          <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">Temperatures</h2>
          <TimeChart series={tempSeries()} height={260} yUnit="°C" syncKey={SYNC_KEY} />
        </Card>
        <div class="mb-4 grid gap-4 sm:grid-cols-2">
          <BigStat label={`Odometer (${unit()})`} value={odometerText()} />
        </div>
      </Show>
    </div>
  )
}
