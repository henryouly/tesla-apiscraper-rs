import { Show, createMemo, createResource, onCleanup, onMount } from 'solid-js'
import { A, useSearchParams } from '@solidjs/router'
import L from 'leaflet'
import 'leaflet/dist/leaflet.css'
import { useDashboardScope } from '../components/DashboardsLayout'
import { BigStat, DataTable, type DataColumn } from '../components/data'
import { Alert, Card, EmptyState, Spinner } from '../components/ui'
import { TimeChart, type ChartSeries } from '../lib/charts'
import { ApiError } from '../lib/api'
import { fetchChargeDetails, fetchChargingStats, type TopStation } from '../lib/dashboards'

interface StationTableRow extends Record<string, unknown> {
  station: string
  energy_kwh: number
  sessions: number
}

function toRow(r: TopStation): StationTableRow {
  return { station: r.station, energy_kwh: r.energy_kwh, sessions: r.sessions }
}

// Cost formatting follows the Charges page conventions: raw numbers from the
// backend rendered with two decimals (no currency symbol).
function costText(v: number | null): string {
  return v == null ? '—' : v.toFixed(2)
}

// Humanized session length: minutes below an hour, "1h 30m" above it.
function durationText(s: number | null): string {
  if (s == null) return '—'
  const mins = Math.round(s / 60)
  if (mins < 60) return `${mins} min`
  const h = Math.floor(mins / 60)
  const m = mins % 60
  return m === 0 ? `${h}h` : `${h}h ${m}m`
}

// Session location marker. Same OSM tile + circleMarker setup as the
// Geofences page — deliberately no L.marker (its default icon images are
// bundler-hostile).
function ChargeMap(props: { lat: number; lng: number }) {
  let mapEl!: HTMLDivElement
  let map: L.Map | undefined
  onMount(() => {
    map = L.map(mapEl).setView([props.lat, props.lng], 14)
    L.tileLayer('https://tile.openstreetmap.org/{z}/{x}/{y}.png', {
      maxZoom: 19,
      attribution: '&copy; OpenStreetMap contributors',
    }).addTo(map)
    L.circleMarker([props.lat, props.lng], {
      radius: 8,
      color: '#e82127',
      fillColor: '#e82127',
      fillOpacity: 0.9,
    }).addTo(map)
  })
  onCleanup(() => map?.remove())
  return (
    <Card class="mb-4 p-5">
      <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">Location</h2>
      <div class="overflow-hidden rounded-xl ring-1 ring-zinc-900/10 dark:ring-white/10">
        <div ref={(el) => (mapEl = el)} class="h-64 w-full" />
      </div>
    </Card>
  )
}

export function ChargingStats() {
  const scope = useDashboardScope()
  const range = () => scope.range()
  const vin = () => scope.vin()
  const [params] = useSearchParams()
  const chargeId = (): string | null => {
    const v = params.charge_id
    return typeof v === 'string' && v.length > 0 ? v : null
  }

  // Period mode is untouched when no charge_id is given; the range fetch
  // is skipped in detail mode (one session has no window).
  const [stats] = createResource(
    () => {
      if (chargeId() != null) return false
      const v = vin()
      if (v == null) return false
      return { vin: v, range: range() }
    },
    (s) => fetchChargingStats({ vin: s.vin, from: s.range.from, to: s.range.to }),
  )

  const [detail] = createResource(chargeId, (id) => fetchChargeDetails(id))
  const isNotFound = () => detail.error instanceof ApiError && detail.error.status === 404
  const hasLocation = () => detail()?.lat != null && detail()?.lng != null

  const rows = createMemo(() => (stats()?.top_stations ?? []).map(toRow))
  const isEmpty = () =>
    (stats()?.count ?? 0) === 0 &&
    (stats()?.soc_delta.length ?? 0) === 0 &&
    (stats()?.top_stations.length ?? 0) === 0

  const energyText = () => `${(stats()?.energy_kwh ?? 0).toFixed(1)} kWh`
  const totalCostText = () => costText(stats()?.total_cost ?? 0)
  const avgCostText = () => {
    const v = stats()?.avg_cost_per_kwh
    return v == null ? '—' : v.toFixed(2)
  }

  const detailEnergyText = () => {
    const v = detail()?.energy_kwh
    return v == null ? '—' : `${v.toFixed(1)} kWh`
  }
  const detailSocText = () => {
    const v = detail()?.soc_added
    return v == null ? '—' : `${Math.round(v)}%`
  }

  const columns = createMemo((): Array<DataColumn<StationTableRow>> => [
    { key: 'station', label: 'Station' },
    {
      key: 'energy_kwh',
      label: 'Energy (kWh)',
      align: 'right',
      render: (row) => row.energy_kwh.toFixed(1),
    },
    {
      key: 'sessions',
      label: 'Sessions',
      align: 'right',
      render: (row) => String(row.sessions),
    },
  ])

  const socSeries = createMemo((): Array<ChartSeries> => [
    {
      label: 'Start SOC',
      data: (stats()?.soc_delta ?? []).map((p): [number, number | null] => [p.t, p.start]),
    },
    {
      label: 'End SOC',
      data: (stats()?.soc_delta ?? []).map((p): [number, number | null] => [p.t, p.end]),
    },
  ])

  const batterySeries = createMemo((): Array<ChartSeries> => [
    {
      label: 'Battery',
      data: (detail()?.curve ?? []).map((p): [number, number | null] => [p.t, p.battery]),
      color: '#10b981',
    },
  ])

  const powerSeries = createMemo((): Array<ChartSeries> => [
    {
      label: 'Power',
      data: (detail()?.curve ?? []).map((p): [number, number | null] => [p.t, p.power_kw]),
      color: '#e82127',
    },
  ])

  return (
    <div>
      <A
        href="/dashboards/charges"
        class="mb-4 inline-block text-sm font-medium text-zinc-500 hover:text-zinc-900 dark:text-zinc-400 dark:hover:text-zinc-100"
      >
        ← Back to charges
      </A>
      <Show when={vin()}>
        <Show when={chargeId() == null}>
          <Show when={stats.loading && !stats()}>
            <Spinner />
          </Show>
          <Show when={stats.error}>
            <div class="mb-4">
              <Alert tone="error">
                {stats.error instanceof Error ? stats.error.message : 'Failed to load charging stats data.'}
              </Alert>
            </div>
          </Show>
          <Show when={!stats.loading && !stats.error && stats() && isEmpty()}>
            <EmptyState
              title="No charges for this period"
              hint="Try a wider time range, or check back once the car completes a charge."
            />
          </Show>
          <Show when={!stats.error && stats() && !isEmpty()}>
            <div class="mb-4 grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
              <BigStat label="# of Charges" value={String(stats()?.count ?? 0)} />
              <BigStat label="Energy Added" value={energyText()} />
              <BigStat label="Total Cost" value={totalCostText()} />
              <BigStat label="Avg Cost / kWh" value={avgCostText()} />
            </div>
            <Card class="mb-4 p-5">
              <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">Charge Level Delta</h2>
              <TimeChart series={socSeries()} height={260} yUnit="%" />
            </Card>
            <Card class="mb-4 p-5">
              <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">Top Charging Stations</h2>
              <DataTable columns={columns()} rows={rows()} />
            </Card>
          </Show>
        </Show>
        <Show when={chargeId() != null}>
          <Show when={detail.loading && !detail()}>
            <Spinner />
          </Show>
          <Show when={isNotFound()}>
            <EmptyState
              title="Charge not found"
              hint="This charging session no longer exists, or the link is stale."
            />
          </Show>
          <Show when={!isNotFound() && detail.error}>
            <div class="mb-4">
              <Alert tone="error">
                {detail.error instanceof Error ? detail.error.message : 'Failed to load charge details.'}
              </Alert>
            </div>
          </Show>
          <Show when={!detail.error && detail()}>
            <div class="mb-4 grid gap-4 sm:grid-cols-2 lg:grid-cols-5">
              <BigStat label="Cost" value={costText(detail()?.cost ?? null)} />
              <BigStat label="Duration" value={durationText(detail()?.duration_s ?? null)} />
              <BigStat label="Location" value={detail()?.geofence ?? '—'} />
              <BigStat label="Energy" value={detailEnergyText()} />
              <BigStat label="SOC Added" value={detailSocText()} />
            </div>
            <Show when={hasLocation()}>
              <ChargeMap lat={detail()!.lat!} lng={detail()!.lng!} />
            </Show>
            <Card class="mb-4 p-5">
              <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">Battery Level</h2>
              <TimeChart series={batterySeries()} height={260} yUnit="%" />
            </Card>
            <Card class="mb-4 p-5">
              <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">Charging Power</h2>
              <TimeChart series={powerSeries()} height={260} yUnit="kW" />
            </Card>
          </Show>
        </Show>
      </Show>
    </div>
  )
}
