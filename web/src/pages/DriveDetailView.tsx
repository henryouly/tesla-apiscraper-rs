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

// Panel geometry for the 2x2 grid: one declared row height that every cell
// fills, so no cell has to be sized by a sibling. The map needs it because
// Leaflet measures its panes once at init and never re-measures — a row height
// that depends on a sibling's content can leave the map init against a shorter
// box than it ends up occupying, leaving the bottom unpainted.
//
// 365 = a chart cell's parts: 2 border + 40 p-5 + 32 heading (20 + mb-3 12) +
// 25 legend row + 266 plot. Charts that fill measure that row rather than
// recompute it, so a chart whose legend is floated (the Drive chart's tooltip)
// simply spends those 25px on plot instead, and the map gets the whole row.
const ROW_H = 365

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
  // Full bleed: no padding, no heading, no margin — the cell is entirely map.
  // `h-full` resolves against the grid's declared row height, so the container
  // is already its final size when Leaflet measures it on mount.
  return (
    <Card class="overflow-hidden p-0">
      <div ref={(el) => (mapEl = el)} class="drive-map h-full w-full" />
    </Card>
  )
}

// Stands in for the map when the drive logged no positions, so the grid keeps
// its 2x2 shape instead of leaving a hole top-right. Stretches with the grid
// and centres its message, matching the full-bleed map cell it replaces.
// Mirrors the charts' own "No data." fallback styling.
function RouteMissing() {
  return (
    <Card class="flex items-center justify-center p-5">
      <p class="text-sm text-zinc-500 dark:text-zinc-400">No route data for this drive.</p>
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
  // Speeds read "mph" in imperial (not "mi/h") and "km/h" in metric — same as
  // the Drives page's speedUnit.
  const speedUnit = () => (unit() === 'mi' ? 'mph' : 'km/h')

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
    return v == null ? '—' : `${round1(v)} ${speedUnit()}`
  }
  const odometerText = () => {
    const o = detail()?.odometer
    if (o?.from == null || o?.to == null) return '—'
    return `${round1(o.from)} → ${round1(o.to)} ${unit()}`
  }
  // Wh per mile driven, from the drive's net energy — rounded to match the
  // Drives table's `Ø Consumption` column. The distance unit is left off the
  // value (the label carries it, and the setting is global); "Wh" alone keeps
  // the card readable next to the kWh figure.
  const consumptionText = () => {
    const e = detail()?.stats.energy_kwh
    const d = detail()?.stats.distance
    if (e == null || d == null || d <= 0) return '—'
    return `${Math.round((e * 1000) / d)} Wh`
  }

  // The Drive chart mixes units (speed, kW, %, range), so it carries no
  // single y-axis unit — same as the Grafana panel's default short unit.
  // Only the four signals the drive_details series selects (speed, power,
  // battery_level, ideal_battery_range_km) can be plotted here. Each series
  // carries its own unit so the tooltip can label what the shared y-axis
  // cannot.
  const driveSeries = createMemo((): Array<ChartSeries> => [
    {
      label: 'Speed',
      data: (detail()?.series ?? []).map((p): [number, number | null] => [p.t, p.speed]),
      color: '#e82127',
      unit: speedUnit(),
    },
    {
      label: 'Power',
      data: (detail()?.series ?? []).map((p): [number, number | null] => [p.t, p.power_kw]),
      color: '#0ea5e9',
      unit: 'kW',
    },
    {
      label: 'SOC',
      data: (detail()?.series ?? []).map((p): [number, number | null] => [p.t, p.battery]),
      color: '#10b981',
      unit: '%',
    },
    {
      label: 'Range (est.)',
      data: (detail()?.series ?? []).map((p): [number, number | null] => [p.t, p.ideal]),
      color: '#f59e0b',
      unit: unit(),
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
      unit: '°C',
    },
    {
      label: 'Inside',
      data: (detail()?.temps ?? []).map((p): [number, number | null] => [p.t, p.inside]),
      color: '#e82127',
      unit: '°C',
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
        {/* 2x2 panel grid, matching the Grafana `drive-details.json` gridPos
            (Drive/Route on top, Elevation/Temperatures beneath). The charts
            share one crosshair, so reading them side by side is the point.
            Declared row height (see ROW_H) — content-sized rows would leave
            the map's box undetermined when Leaflet measures it. */}
        <div class="mb-4 grid gap-4 lg:grid-cols-2" style={{ 'grid-auto-rows': `${ROW_H}px` }}>
          <Card class="flex min-h-0 flex-col p-5">
            <h2 class="mb-3 shrink-0 text-sm font-semibold text-zinc-700 dark:text-zinc-200">
              Drive
            </h2>
            <div class="min-h-0 flex-1">
              <TimeChart series={driveSeries()} fill tooltip syncKey={SYNC_KEY} />
            </div>
          </Card>
          <Show when={hasRoute()} fallback={<RouteMissing />}>
            <RouteMap route={detail()!.route} />
          </Show>
          <Card class="flex min-h-0 flex-col p-5">
            <h2 class="mb-3 shrink-0 text-sm font-semibold text-zinc-700 dark:text-zinc-200">
              Elevation
            </h2>
            <div class="min-h-0 flex-1">
              <TimeChart series={elevationSeries()} fill yUnit="m" syncKey={SYNC_KEY} />
            </div>
          </Card>
          <Card class="flex min-h-0 flex-col p-5">
            <h2 class="mb-3 shrink-0 text-sm font-semibold text-zinc-700 dark:text-zinc-200">
              Temperatures
            </h2>
            <div class="min-h-0 flex-1">
              <TimeChart series={tempSeries()} fill tooltip yUnit="°C" syncKey={SYNC_KEY} />
            </div>
          </Card>
        </div>
        <div class="mb-4 grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
          <BigStat label="Odometer" value={odometerText()} />
          <BigStat label="Drive Duration" value={durationText(detail()?.stats.duration_s ?? null)} />
          <BigStat label="Distance Driven" value={distanceText()} />
          <BigStat label="Energy consumed" value={energyText()} />
          <BigStat label="Consumption (mile average)" value={consumptionText()} />
          <BigStat label="Avg speed" value={avgSpeedText()} />
        </div>
      </Show>
    </div>
  )
}
