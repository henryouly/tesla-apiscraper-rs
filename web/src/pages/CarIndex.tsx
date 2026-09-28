import { For, Show, createEffect, createResource, createSignal, onCleanup } from 'solid-js'
import L from 'leaflet'
import 'leaflet/dist/leaflet.css'
import { ApiError, api, type VehicleSummary } from '../lib/api'
import { useUnits } from '../lib/units'
import { useSse, type SseStatus } from '../lib/sse'
import { Button, Card, Spinner } from '../components/ui'

function fmtTime(unix: number): string {
  if (!unix) return 'never'
  return new Date(unix * 1000).toLocaleString()
}

// Mirror of VehicleSummary::has_telemetry (src/vehicle_summary.rs) —
// keep the field sets in sync.
function hasTelemetry(s: VehicleSummary): boolean {
  return (
    s.battery_level != null ||
    s.battery_range != null ||
    s.latitude != null ||
    s.longitude != null ||
    s.speed != null ||
    s.odometer != null
  )
}

function statusOf(car: VehicleSummary): string {
  return car.charging_state ?? car.shift_state ?? car.state
}

function CarCard(props: {
  car: VehicleSummary
  onChanged: () => void
  flash: (m: string) => void
}) {
  const units = useUnits()
  const [busy, setBusy] = createSignal(false)
  const suspended = () => props.car.state === 'Suspended'

  const toggle = async () => {
    setBusy(true)
    try {
      if (suspended()) await api.resume(props.car.vin)
      else await api.suspend(props.car.vin)
      props.onChanged()
    } catch (err) {
      props.flash(err instanceof ApiError ? err.body : 'Request failed')
    } finally {
      setBusy(false)
    }
  }

  // Last-known-location mini map. One Leaflet instance per card; the
  // marker and view follow live SSE updates via the effect below.
  let mapEl!: HTMLDivElement
  let map: L.Map | undefined
  let marker: L.CircleMarker | undefined
  const hasLoc = () => props.car.latitude != null && props.car.longitude != null

  // The effect creates the map lazily on first coordinates, so a car
  // that wakes up after first paint still gets its map, then follows
  // live SSE updates. Refs are set before effects run.
  createEffect(() => {
    if (!hasLoc()) return
    const pos: [number, number] = [props.car.latitude!, props.car.longitude!]
    if (!map || !marker) {
      map = L.map(mapEl).setView(pos, 13)
      L.tileLayer('https://tile.openstreetmap.org/{z}/{x}/{y}.png', {
        maxZoom: 19,
        attribution: '&copy; OpenStreetMap contributors',
      }).addTo(map)
      marker = L.circleMarker(pos, {
        radius: 8,
        color: '#2563eb',
        fillColor: '#2563eb',
        fillOpacity: 0.9,
      }).addTo(map)
      return
    }
    marker.setLatLng(pos)
    map.setView(pos)
  })

  onCleanup(() => map?.remove())

  const charging = () => props.car.charging_state === 'Charging'

  return (
    <Card>
      <div class="mb-2 flex items-center justify-between">
        <h2 class="text-lg font-bold">
          {props.car.display_name || props.car.vin}
        </h2>
        <span class="rounded bg-gray-200 px-2 py-0.5 text-xs font-medium dark:bg-gray-700">
          {props.car.state}
        </span>
      </div>
      <Show
        when={hasTelemetry(props.car)}
        fallback={
          <p class="text-sm text-gray-500">
            Waiting for first data — the car may be asleep or offline.
          </p>
        }
      >
        <Show
          when={hasLoc()}
          fallback={<p class="text-sm text-gray-500">Location unknown.</p>}
        >
          <div ref={(el) => (mapEl = el)} class="mb-1 h-48 w-full rounded" />
          <Show when={props.car.geofence_name}>
            <p class="mb-2 text-xs text-gray-500">{props.car.geofence_name}</p>
          </Show>
        </Show>
        <dl class="grid grid-cols-2 gap-x-4 gap-y-1 text-sm">
          <dt class="text-gray-500">Status</dt>
          <dd>{statusOf(props.car)}</dd>
          <Show when={charging()}>
            <dt class="text-gray-500">Time to full</dt>
            <dd>{units.formatDurationHours(props.car.time_to_full_charge)}</dd>
          </Show>
          <dt class="text-gray-500">Range (ideal)</dt>
          <dd>{units.formatMiles(props.car.ideal_battery_range)}</dd>
          <dt class="text-gray-500">Range (est.)</dt>
          <dd>{units.formatMiles(props.car.est_battery_range)}</dd>
          <Show when={charging()}>
            <dt class="text-gray-500">Charging power</dt>
            <dd>{props.car.charger_power != null ? `${props.car.charger_power} kW` : '—'}</dd>
            <dt class="text-gray-500">Charged added</dt>
            <dd>
              {props.car.charge_energy_added != null ? `${props.car.charge_energy_added} kWh` : '—'}
            </dd>
          </Show>
          <dt class="text-gray-500">Charge limit</dt>
          <dd>{props.car.charge_limit_soc != null ? `${props.car.charge_limit_soc}%` : '—'}</dd>
          <dt class="text-gray-500">State of charge</dt>
          <dd>{props.car.battery_level != null ? `${props.car.battery_level}%` : '—'}</dd>
          <dt class="text-gray-500">Outside temp</dt>
          <dd>{units.formatTemp(props.car.outside_temp)}</dd>
          <dt class="text-gray-500">Inside temp</dt>
          <dd>{units.formatTemp(props.car.inside_temp)}</dd>
          <dt class="text-gray-500">Mileage</dt>
          <dd>{units.formatMiles(props.car.odometer)}</dd>
          <dt class="text-gray-500">Version</dt>
          <dd>{props.car.car_version ?? '—'}</dd>
          <dt class="text-gray-500">Updated</dt>
          <dd>{fmtTime(props.car.last_updated_at)}</dd>
        </dl>
      </Show>
      <div class="mt-3">
        <Button onClick={toggle} disabled={busy()} variant="ghost">
          {busy() ? '…' : suspended() ? 'Resume logging' : 'Suspend logging'}
        </Button>
      </div>
    </Card>
  )
}

export function CarIndex() {
  const [data, { refetch }] = createResource(() => api.summaries())
  const [discovery, { refetch: refetchDiscovery }] = createResource(() => api.vehicles())
  const [cars, setCars] = createSignal<Map<string, VehicleSummary>>(new Map())
  const [sseStatus, setSseStatus] = createSignal<SseStatus>('connecting')
  const [flash, setFlash] = createSignal<string | null>(null)

  // Seed from the initial fetch, then keep live via SSE. Merge per-VIN by
  // server timestamp, keeping the live value on ties: timestamps have
  // one-second precision, so a fetch resolving after a streaming update can
  // carry the same stamp as newer card data.
  const mergeSummaries = (incoming: VehicleSummary[]) => {
    setCars((m) => {
      const next = new Map(m)
      for (const s of incoming) {
        const cur = next.get(s.vin)
        if (!cur || s.last_updated_at > cur.last_updated_at) next.set(s.vin, s)
      }
      return next
    })
  }
  createEffect(() => {
    const d = data()
    if (d) mergeSummaries(d.summaries)
  })

  const refetchAll = () => {
    refetch()
    refetchDiscovery()
  }

  useSse(
    (ev) => {
      if (ev.type === 'summary') {
        setCars((m) => new Map(m).set(ev.vin, ev.summary))
      } else if (ev.type === 'state') {
        setCars((m) => {
          const cur = m.get(ev.vin)
          if (!cur) return m
          const next = new Map(m)
          next.set(ev.vin, { ...cur, state: ev.state })
          return next
        })
      }
    },
    refetchAll,
    setSseStatus,
  )

  const list = () => [...cars().values()]
  const knownCount = () => discovery()?.vehicles.length ?? 0

  return (
    <div>
      <div class="mb-4 flex items-center gap-2">
        <h1 class="text-xl font-bold">Cars</h1>
        <span class="text-xs text-gray-500">
          {sseStatus() === 'live'
            ? '● live'
            : sseStatus() === 'reconnecting'
              ? '● reconnecting…'
              : '● connecting…'}
        </span>
      </div>
      <Show when={flash()}>
        <p class="mb-3 text-sm text-red-600">{flash()}</p>
      </Show>
      <Show when={data.loading}>
        <Spinner />
      </Show>
      <Show when={data.error}>
        <p class="text-sm text-red-600">Failed to load vehicles.</p>
      </Show>
      <Show when={!data.loading && !data.error && list().length === 0 && knownCount() === 0}>
        <p class="text-sm text-gray-500">No vehicles on this Tesla account.</p>
      </Show>
      <Show when={!data.loading && !data.error && list().length === 0 && knownCount() > 0}>
        <p class="text-sm text-gray-500">
          Waiting for first data from {knownCount() === 1 ? 'your car' : `${knownCount()} cars`}…
        </p>
      </Show>
      <div class="grid gap-4 md:grid-cols-2">
        <For each={list()}>
          {(car) => <CarCard car={car} onChanged={refetchAll} flash={setFlash} />}
        </For>
      </div>
    </div>
  )
}
