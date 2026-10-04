import { For, Show, createEffect, createResource, createSignal, onCleanup } from 'solid-js'
import L from 'leaflet'
import 'leaflet/dist/leaflet.css'
import { A } from '@solidjs/router'
import { ApiError, api, type VehicleSummary } from '../lib/api'
import { useUnits } from '../lib/units'
import { useSse, type SseStatus } from '../lib/sse'
import { Alert, Button, Card, EmptyState, I, Icon, PageHeader, Pill, SkeletonCard, Stat } from '../components/ui'

function fmtTime(unix: number): string {
  if (!unix) return 'never'
  return new Date(unix * 1000).toLocaleString()
}

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

// Raw `charging_state` includes unplugged noise (`Disconnected`, `NoPower`)
// that reads like a connection problem. Only surface real charge activity;
// otherwise fall back to gear / vehicle state.
function chargeStatus(car: VehicleSummary): string | null {
  const cs = car.charging_state
  if (!cs || cs === 'Disconnected' || cs === 'NoPower') return null
  return cs
}

// Backend classifies only D/R as driving (src/vehicles/state.rs); P/N must
// not render a pulsing "Driving" badge on a parked car.
function drivingGear(car: VehicleSummary): string | null {
  const s = car.shift_state
  return s === 'D' || s === 'R' ? s : null
}

function statusPill(car: VehicleSummary) {
  const cs = chargeStatus(car)
  if (cs === 'Charging') return { label: 'Charging', tone: 'green' as const, pulse: true }
  if (cs === 'Starting') return { label: 'Starting', tone: 'blue' as const, pulse: true }
  if (cs) return { label: cs, tone: cs === 'Complete' ? ('green' as const) : ('amber' as const), pulse: false }
  const gear = drivingGear(car)
  if (gear) return { label: `Driving · ${gear}`, tone: 'blue' as const, pulse: true }
  if (car.state === 'Suspended') return { label: 'Suspended', tone: 'amber' as const, pulse: false }
  if (car.state === 'Asleep' || car.state === 'Offline') return { label: car.state, tone: 'gray' as const, pulse: false }
  return { label: car.state, tone: 'gray' as const, pulse: false }
}

function displayStatus(car: VehicleSummary): string {
  const gear = drivingGear(car)
  return chargeStatus(car) ?? (gear ? `Driving · ${gear}` : car.state)
}

// Lock + Sentry for the Status tile, so it doesn't repeat the tracker
// state already shown in the header chip. Sentry implies locked, so the
// lock word is redundant whenever sentry is known: the tile shows
// `Unlocked` (the attention-worthy state), otherwise the sentry state.
// Unknowns are omitted; when both are unknown fall back to displayStatus
// at the call site.
function securityStatus(car: VehicleSummary): string | undefined {
  if (car.locked === false) return 'Unlocked'
  if (car.sentry_mode != null) return car.sentry_mode ? 'Sentry on' : 'Sentry off'
  if (car.locked === true) return 'Locked'
  return undefined
}

function compass(heading: number | null | undefined): string | null {
  if (heading == null) return null
  const dirs = ['N', 'NE', 'E', 'SE', 'S', 'SW', 'W', 'NW']
  const h = ((Math.round(heading) % 360) + 360) % 360
  return `${h}° ${dirs[Math.round(h / 45) % 8]}`
}

function headingSub(car: VehicleSummary, speed: string | null): string | undefined {
  const c = compass(car.heading)
  if (c && speed) return `${speed} · ${c}`
  return c ?? speed ?? undefined
}

function carIcon(heading: number | null | undefined): L.DivIcon {
  if (heading == null) {
    return L.divIcon({
      className: 'car-arrow',
      html: '<div class="car-dot"></div>',
      iconSize: [16, 16],
      iconAnchor: [8, 8],
    })
  }
  const h = ((Math.round(heading) % 360) + 360) % 360
  return L.divIcon({
    className: 'car-arrow',
    html: `<div class="car-nav" style="transform: rotate(${h}deg)"><svg viewBox="0 0 24 24" width="26" height="26" fill="#e82127" stroke="#fff" stroke-width="1.5"><path d="M12 2 L19 19 L12 15.5 L5 19 Z"/></svg></div>`,
    iconSize: [26, 26],
    iconAnchor: [13, 13],
  })
}

function batteryTone(level: number | null): string {
  if (level == null) return 'bg-zinc-600'
  if (level < 15) return 'bg-[#e82127]'
  if (level < 35) return 'bg-amber-400'
  return 'bg-emerald-400'
}

function CarCard(props: { car: VehicleSummary; onChanged: () => void; flash: (m: string) => void }) {
  const units = useUnits()
  const [busy, setBusy] = createSignal(false)
  const suspended = () => props.car.state === 'Suspended'
  const status = () => statusPill(props.car)
  const charging = () => props.car.charging_state === 'Charging'
  const initial = () => (props.car.display_name || props.car.vin || '?').slice(0, 1).toUpperCase()

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

  let mapEl!: HTMLDivElement
  let map: L.Map | undefined
  let marker: L.Marker | undefined
  const hasLoc = () => props.car.latitude != null && props.car.longitude != null

  createEffect(() => {
    if (!hasLoc()) return
    const pos: [number, number] = [props.car.latitude!, props.car.longitude!]
    const heading = props.car.heading
    if (!map || !marker) {
      map = L.map(mapEl, { zoomControl: true, attributionControl: true }).setView(pos, 13)
      L.tileLayer('https://tile.openstreetmap.org/{z}/{x}/{y}.png', {
        maxZoom: 19,
        attribution: '&copy; OpenStreetMap contributors',
      }).addTo(map)
      marker = L.marker(pos, { icon: carIcon(heading) }).addTo(map)
      return
    }
    marker.setLatLng(pos)
    marker.setIcon(carIcon(heading))
    map.setView(pos)
  })

  onCleanup(() => map?.remove())

  return (
    <Card class="overflow-hidden p-0">
      <div class="flex items-center gap-3 border-b border-zinc-200 dark:border-white/[0.06] px-5 py-4">
        <span class="flex h-10 w-10 items-center justify-center rounded-xl bg-gradient-to-br from-[#e82127] to-[#7a1013] text-base font-bold text-white">
          {initial()}
        </span>
        <div class="min-w-0 flex-1">
          <h2 class="truncate text-[15px] font-bold tracking-tight">{props.car.display_name || 'Unnamed car'}</h2>
          <p class="truncate font-mono text-[11px] text-zinc-900 dark:text-zinc-500">{props.car.vin}</p>
        </div>
        <Pill tone={status().tone} pulse={status().pulse}>
          {status().label}
        </Pill>
      </div>

      <Show
        when={hasTelemetry(props.car)}
        fallback={
          <div class="px-5 py-8">
            <EmptyState title="Waiting for first data" hint="The car may be asleep or offline. It will appear here once telemetry arrives." />
          </div>
        }
      >
        <div class="px-5 pt-4">
          <div class="flex items-end justify-between gap-3">
            <div>
              <p class="text-4xl font-bold tracking-tight tabular-nums">
                {props.car.battery_level != null ? `${props.car.battery_level}%` : '—'}
              </p>
              <p class="mt-0.5 text-[13px] text-zinc-500 dark:text-zinc-400">
                {units.formatMiles(props.car.ideal_battery_range)} ideal · {units.formatMiles(props.car.est_battery_range)} est.
              </p>
            </div>
            <div class="flex flex-col items-end gap-1">
              <Show when={props.car.charge_limit_soc != null}>
                <p class="rounded-lg bg-zinc-900/[0.05] dark:bg-white/[0.05] px-2.5 py-1 text-xs text-zinc-500 dark:text-zinc-400 ring-1 ring-inset ring-zinc-900/10 dark:ring-white/10">
                  Limit {props.car.charge_limit_soc}%
                </p>
              </Show>
              <p class="text-[11px] text-zinc-500 dark:text-zinc-400">
                Updated {fmtTime(props.car.last_updated_at)}
              </p>
            </div>
          </div>
          <div class="mt-3 h-2 overflow-hidden rounded-full bg-zinc-900/[0.07] dark:bg-white/[0.07]">
            <div
              class={`h-full rounded-full transition-all duration-700 ${batteryTone(props.car.battery_level)}`}
              style={{ width: `${props.car.battery_level ?? 0}%` }}
            />
          </div>

          <Show when={charging()}>
            <div class="mt-3 flex items-center gap-2 rounded-xl border border-emerald-600/20 bg-emerald-600/[0.07] px-3 py-2 text-[13px] text-emerald-700 dark:border-emerald-500/20 dark:bg-emerald-500/[0.07] dark:text-emerald-200">
              <Icon d={I.bolt} class="h-4 w-4" />
              <span>
                {props.car.charger_power != null ? `${props.car.charger_power} kW` : 'Charging'}
                {props.car.charge_energy_added != null ? ` · +${props.car.charge_energy_added} kWh` : ''}
                {props.car.time_to_full_charge != null ? ` · full in ${units.formatDurationHours(props.car.time_to_full_charge)}` : ''}
              </span>
            </div>
          </Show>
        </div>

        <Show when={hasLoc()} fallback={<p class="px-5 py-4 text-sm text-zinc-900 dark:text-zinc-500">Location unknown.</p>}>
          <div class="px-5 pt-4">
            <div class="relative overflow-hidden rounded-xl ring-1 ring-zinc-900/10 dark:ring-white/10">
              <div ref={(el) => (mapEl = el)} class="h-52 w-full" />
              <Show when={props.car.geofence_name}>
                <span class="absolute right-2.5 top-2.5 z-10 inline-flex items-center gap-1 rounded-full bg-black/70 px-2.5 py-1 text-xs font-medium text-zinc-100 backdrop-blur">
                  <Icon d={I.pin} class="h-3.5 w-3.5 text-[#ff6b6f]" />
                  {props.car.geofence_name}
                </span>
              </Show>
              <Show when={compass(props.car.heading)}>
                <span class="absolute bottom-2.5 left-2.5 z-10 rounded-full bg-black/70 px-2.5 py-1 font-mono text-[11px] font-medium text-zinc-100 backdrop-blur">
                  {compass(props.car.heading)}
                </span>
              </Show>
            </div>
          </div>
        </Show>

        <div class="grid grid-cols-2 gap-2 px-5 py-4 sm:grid-cols-4">
          <Stat label="Odometer" value={units.formatMiles(props.car.odometer)} />
          <Stat label="Outside" value={units.formatTemp(props.car.outside_temp)} />
          <Stat label="Inside" value={units.formatTemp(props.car.inside_temp)} />
          <Stat
            label="Status"
            value={securityStatus(props.car) ?? displayStatus(props.car)}
            sub={headingSub(props.car, props.car.speed != null ? units.formatSpeed(props.car.speed) : null)}
          />
        </div>
      </Show>

      <div class="flex items-center gap-2 border-t border-zinc-200 dark:border-white/[0.06] bg-zinc-900/[0.02] dark:bg-white/[0.02] px-5 py-3">
        <Button onClick={toggle} loading={busy()} variant="secondary" size="sm">
          {suspended() ? 'Resume logging' : 'Suspend logging'}
        </Button>
        <A href={`/settings/car/${encodeURIComponent(props.car.vin)}`} class="inline-flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-xs font-medium text-zinc-500 transition-colors hover:bg-zinc-900/[0.06] hover:text-zinc-900 dark:text-zinc-400 dark:hover:bg-white/[0.06] dark:hover:text-zinc-100">
          <Icon d={I.gear} class="h-3.5 w-3.5" />
          Car settings
        </A>
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
  const live = () => sseStatus() === 'live'

  return (
    <div>
      <PageHeader
        eyebrow="Fleet"
        title="Cars"
        hint={live() ? 'Streaming live telemetry.' : 'Connecting to live telemetry…'}
        actions={
          <Pill tone={live() ? 'green' : 'gray'} pulse={live()}>
            {live() ? 'Live' : sseStatus()}
          </Pill>
        }
      />
      <Show when={flash()}>
        <div class="mb-4">
          <Alert tone="error">{flash()}</Alert>
        </div>
      </Show>
      <Show when={data.loading}>
        <div class="grid gap-4 md:grid-cols-2">
          <SkeletonCard />
          <SkeletonCard />
        </div>
      </Show>
      <Show when={data.error}>
        <Alert tone="error">Failed to load vehicles. Check the server connection and retry.</Alert>
      </Show>
      <Show when={!data.loading && !data.error && list().length === 0 && knownCount() === 0}>
        <EmptyState title="No vehicles on this Tesla account" hint="Sign in with a refresh token if this is a fresh setup." />
      </Show>
      <Show when={!data.loading && !data.error && list().length === 0 && knownCount() > 0}>
        <EmptyState
          title={`Waiting for first data from ${knownCount() === 1 ? 'your car' : `${knownCount()} cars`}…`}
          hint="Cars report in once they wake up and the poller collects telemetry."
        />
      </Show>
      <div class="grid items-start gap-4 md:grid-cols-2">
        <For each={list()}>{(car) => <CarCard car={car} onChanged={refetchAll} flash={setFlash} />}</For>
      </div>
    </div>
  )
}
