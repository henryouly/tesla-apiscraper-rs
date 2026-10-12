import { A, useParams } from '@solidjs/router'
import { Show, createEffect, createMemo, createResource, onCleanup } from 'solid-js'
import L from 'leaflet'
import 'leaflet/dist/leaflet.css'
import { BigStat } from '../components/data'
import { Alert, Card, EmptyState, Spinner } from '../components/ui'
import { TimeChart, type ChartSeries } from '../lib/charts'
import { ApiError } from '../lib/api'
import { fetchChargeDetails } from '../lib/dashboards'

// Cost formatting follows the Charges/ChargeCost conventions: raw backend
// numbers rendered with two decimals (no currency symbol).
function costText(v: number | null): string {
  return v == null ? '—' : v.toFixed(2)
}

function durationText(seconds: number | null): string {
  if (seconds == null) return '—'
  const total = Math.round(seconds)
  const h = Math.floor(total / 3600)
  const m = Math.floor((total % 3600) / 60)
  const s = total % 60
  if (h > 0) return m > 0 ? `${h}h ${m}m` : `${h}h`
  if (m > 0) return `${m}m`
  return `${s}s`
}

export function ChargeDetails() {
  const params = useParams()
  const id = () => decodeURIComponent(params.id ?? '')

  const [details] = createResource(id, (charge_id) => fetchChargeDetails(charge_id))

  const isNotFound = () => details.error instanceof ApiError && details.error.status === 404

  const curveSeries = createMemo((): Array<ChartSeries> => {
    const curve = details()?.curve ?? []
    return [
      {
        label: 'Battery %',
        data: curve.map((p): [number, number | null] => [p.t, p.battery]),
        color: '#10b981',
      },
      {
        label: 'Power kW',
        data: curve.map((p): [number, number | null] => [p.t, p.power_kw]),
        color: '#e82127',
      },
      {
        label: 'Added kWh',
        data: curve.map((p): [number, number | null] => [p.t, p.added_kwh]),
        color: '#0ea5e9',
      },
    ]
  })

  // Leaflet marker map, same setup as CarIndex/Geofences (OSM tiles).
  // A circleMarker is used instead of the default marker icon so no
  // external image assets are needed (same reason Geofences avoids
  // L.marker; CarIndex uses a DivIcon for the same purpose).
  let mapEl!: HTMLDivElement
  let map: L.Map | undefined
  let marker: L.CircleMarker | undefined
  const pos = (): [number, number] | null => {
    const d = details()
    return d?.lat != null && d?.lng != null ? [d.lat, d.lng] : null
  }

  createEffect(() => {
    const p = pos()
    if (p == null) return
    if (!map || !marker) {
      map = L.map(mapEl, { zoomControl: true, attributionControl: true }).setView(p, 13)
      L.tileLayer('https://tile.openstreetmap.org/{z}/{x}/{y}.png', {
        maxZoom: 19,
        attribution: '&copy; OpenStreetMap contributors',
      }).addTo(map)
      marker = L.circleMarker(p, {
        radius: 8,
        color: '#e82127',
        fillColor: '#e82127',
        fillOpacity: 0.9,
      }).addTo(map)
      return
    }
    marker.setLatLng(p)
    map.setView(p)
  })

  onCleanup(() => map?.remove())

  return (
    <div>
      <A
        href="/dashboards/charges"
        class="mb-4 inline-block text-sm font-medium text-zinc-500 hover:text-zinc-900 dark:text-zinc-400 dark:hover:text-zinc-100"
      >
        ← Back to charges
      </A>
      <Show when={details.loading && !details()}>
        <Spinner />
      </Show>
      <Show when={isNotFound()}>
        <EmptyState title="Charge not found" hint="This charging session does not exist." />
      </Show>
      <Show when={!isNotFound() && details.error}>
        <div class="mb-4">
          <Alert tone="error">
            {details.error instanceof Error ? details.error.message : 'Failed to load charge details.'}
          </Alert>
        </div>
      </Show>
      <Show when={!details.error && details()}>
        <div class="mb-4 grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
          <BigStat label="Cost" value={costText(details()?.cost ?? null)} />
          <BigStat label="Duration" value={durationText(details()?.duration_s ?? null)} />
          <BigStat label="Location" value={details()?.geofence ?? '—'} />
          <BigStat
            label="Energy Added"
            value={details()?.energy_kwh == null ? '—' : `${details()!.energy_kwh!.toFixed(1)} kWh`}
          />
          <BigStat
            label="SOC Added"
            value={details()?.soc_added == null ? '—' : `${Math.round(details()!.soc_added!)}%`}
          />
        </div>
        <Show when={pos()}>
          <Card class="mb-4 p-5">
            <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">Location</h2>
            <div class="overflow-hidden rounded-xl ring-1 ring-zinc-900/10 dark:ring-white/10">
              <div ref={(el) => (mapEl = el)} class="h-64 w-full" />
            </div>
          </Card>
        </Show>
        <Card class="mb-4 p-5">
          <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">Charge Curve</h2>
          <TimeChart series={curveSeries()} height={260} />
        </Card>
      </Show>
    </div>
  )
}
