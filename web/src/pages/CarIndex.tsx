import { For, Show, createEffect, createResource, createSignal } from 'solid-js'
import { ApiError, api, type VehicleSummary } from '../lib/api'
import { useSse, type SseStatus } from '../lib/sse'
import { Button, Card, Spinner } from '../components/ui'

function fmtTime(unix: number): string {
  if (!unix) return 'never'
  return new Date(unix * 1000).toLocaleString()
}

function fmtLoc(s: VehicleSummary): string {
  if (s.latitude == null || s.longitude == null) return '—'
  return `${s.latitude.toFixed(4)}, ${s.longitude.toFixed(4)}`
}

function hasTelemetry(s: VehicleSummary): boolean {
  return s.battery_level != null || s.latitude != null || s.odometer != null
}

function CarCard(props: {
  car: VehicleSummary
  onChanged: () => void
  flash: (m: string) => void
}) {
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
        <dl class="grid grid-cols-2 gap-x-4 gap-y-1 text-sm">
          <dt class="text-gray-500">Battery</dt>
          <dd>{props.car.battery_level != null ? `${props.car.battery_level}%` : '—'}</dd>
          <dt class="text-gray-500">Range</dt>
          <dd>{props.car.battery_range != null ? `${props.car.battery_range.toFixed(0)} mi` : '—'}</dd>
          <dt class="text-gray-500">Location</dt>
          <dd>{fmtLoc(props.car)}</dd>
          <dt class="text-gray-500">Speed</dt>
          <dd>{props.car.speed != null ? `${props.car.speed.toFixed(0)} mph` : '—'}</dd>
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

  // Seed from the initial fetch, then keep live via SSE.
  createEffect(() => {
    const d = data()
    if (d) setCars(new Map(d.summaries.map((s) => [s.vin, s])))
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
