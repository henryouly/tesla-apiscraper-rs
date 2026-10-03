import {
  For,
  Show,
  createEffect,
  createResource,
  createSignal,
  onCleanup,
  onMount,
} from 'solid-js'
import L from 'leaflet'
import 'leaflet/dist/leaflet.css'
import { ApiError, api, type Geofence } from '../lib/api'
import { parseNumber } from '../lib/number'
import { Alert, Button, Card, EmptyState, FormField, I, Icon, PageHeader, Pill, Select, Spinner } from '../components/ui'

const emptyForm = (): Geofence => ({
  name: '',
  latitude: 37.7749,
  longitude: -122.4194,
  radius_meters: 100,
  billing: null,
})

function billingLabel(g: Geofence): string {
  const b = g.billing
  if (!b) return 'no billing'
  const unit = b.type === 'per_kwh' ? 'kWh' : 'min'
  return `${b.cost_per_unit}/${unit} + ${b.session_fee} fee`
}

export function Geofences() {
  const [list, { refetch }] = createResource(() => api.geofences())
  const [form, setForm] = createSignal<Geofence>(emptyForm())
  const [editing, setEditing] = createSignal<string | null>(null)
  const [latText, setLatText] = createSignal(String(emptyForm().latitude))
  const [lngText, setLngText] = createSignal(String(emptyForm().longitude))
  const [radiusText, setRadiusText] = createSignal(String(emptyForm().radius_meters))
  const [billingMode, setBillingMode] = createSignal<'none' | 'per_kwh' | 'per_minute'>('none')
  const [rate, setRate] = createSignal('0.3')
  const [fee, setFee] = createSignal('0')
  const [search, setSearch] = createSignal('')
  const [error, setError] = createSignal<string | null>(null)
  const [busy, setBusy] = createSignal(false)
  const [confirmDelete, setConfirmDelete] = createSignal<string | null>(null)

  let mapEl!: HTMLDivElement
  let map: L.Map | undefined
  let marker: L.CircleMarker | undefined
  let circle: L.Circle | undefined

  const patch = (p: Partial<Geofence>) => setForm((f) => ({ ...f, ...p }))

  onMount(() => {
    map = L.map(mapEl).setView([form().latitude, form().longitude], 13)
    L.tileLayer('https://tile.openstreetmap.org/{z}/{x}/{y}.png', {
      maxZoom: 19,
      attribution: '&copy; OpenStreetMap contributors',
    }).addTo(map)
    marker = L.circleMarker([form().latitude, form().longitude], {
      radius: 8,
      color: '#e82127',
      fillColor: '#e82127',
      fillOpacity: 0.9,
    }).addTo(map)
    circle = L.circle([form().latitude, form().longitude], {
      radius: form().radius_meters,
      color: '#e82127',
      weight: 1.5,
      fillOpacity: 0.08,
    }).addTo(map)
    map.on('click', (e: L.LeafletMouseEvent) => {
      const lat = +e.latlng.lat.toFixed(6)
      const lng = +e.latlng.lng.toFixed(6)
      patch({ latitude: lat, longitude: lng })
      setLatText(String(lat))
      setLngText(String(lng))
    })
  })

  createEffect(() => {
    const lat = parseNumber(latText())
    const lng = parseNumber(lngText())
    const r = parseNumber(radiusText())
    if (lat == null || lng == null) return
    marker?.setLatLng([lat, lng])
    circle?.setLatLng([lat, lng])
    circle?.setRadius(r != null && r > 0 ? r : 0)
  })

  onCleanup(() => map?.remove())

  const doSearch = async () => {
    const q = search().trim()
    if (!q) return
    try {
      const resp = await fetch(
        `https://nominatim.openstreetmap.org/search?format=json&limit=1&q=${encodeURIComponent(q)}`,
      )
      const [hit] = (await resp.json()) as Array<{ lat: string; lon: string }>
      if (!hit) {
        setError('Place not found')
        return
      }
      const lat = +(+hit.lat).toFixed(6)
      const lng = +(+hit.lon).toFixed(6)
      patch({ latitude: lat, longitude: lng })
      setLatText(String(lat))
      setLngText(String(lng))
      map?.setView([lat, lng], 14)
      setError(null)
    } catch {
      setError('Search failed')
    }
  }

  const startEdit = (g: Geofence) => {
    setForm({ ...g })
    setEditing(g.name)
    setError(null)
    setLatText(String(g.latitude))
    setLngText(String(g.longitude))
    setRadiusText(String(g.radius_meters))
    if (g.billing) {
      setBillingMode(g.billing.type)
      setRate(String(g.billing.cost_per_unit))
      setFee(String(g.billing.session_fee))
    } else {
      setBillingMode('none')
      setRate('0.3')
      setFee('0')
    }
    map?.setView([g.latitude, g.longitude], 14)
    window.scrollTo({ top: 0, behavior: 'smooth' })
  }

  const startCreate = () => {
    const fresh = emptyForm()
    setForm(fresh)
    setEditing(null)
    setLatText(String(fresh.latitude))
    setLngText(String(fresh.longitude))
    setRadiusText(String(fresh.radius_meters))
    setBillingMode('none')
    setRate('0.3')
    setFee('0')
    setError(null)
  }

  const submit = async (e: Event) => {
    e.preventDefault()
    const latitude = parseNumber(latText())
    const longitude = parseNumber(lngText())
    const radius_meters = parseNumber(radiusText())
    if (latitude == null || longitude == null || radius_meters == null) {
      setError('Latitude, longitude, and radius must be numbers')
      return
    }
    const cost_per_unit = parseNumber(rate())
    const session_fee = parseNumber(fee())
    if (billingMode() !== 'none' && (cost_per_unit == null || session_fee == null)) {
      setError('Billing amounts must be numbers')
      return
    }
    setBusy(true)
    setError(null)
    const f = form()
    const payload: Geofence = {
      ...f,
      latitude,
      longitude,
      radius_meters,
      billing:
        billingMode() === 'none'
          ? null
          : {
              type: billingMode() as 'per_kwh' | 'per_minute',
              cost_per_unit: cost_per_unit ?? 0,
              session_fee: session_fee ?? 0,
            },
    }
    try {
      if (editing()) await api.updateGeofence(editing()!, payload)
      else await api.createGeofence(payload)
      startCreate()
      refetch()
    } catch (err) {
      setError(err instanceof ApiError ? err.body : 'Save failed')
    } finally {
      setBusy(false)
    }
  }

  const remove = async (name: string) => {
    if (confirmDelete() !== name) {
      setConfirmDelete(name)
      return
    }
    setConfirmDelete(null)
    try {
      await api.deleteGeofence(name)
      if (editing() === name) startCreate()
      refetch()
    } catch (err) {
      setError(err instanceof ApiError ? err.body : 'Delete failed')
    }
  }

  const fences = () => list()?.geofences ?? []

  return (
    <div>
      <PageHeader
        eyebrow="Places"
        title="Geofences"
        hint="Click the map to place the center. Billing applies to future charging sessions only."
        actions={
          <Show when={editing()}>
            <Button variant="secondary" size="sm" onClick={startCreate}>
              <Icon d={I.plus} class="h-3.5 w-3.5" /> New fence
            </Button>
          </Show>
        }
      />
      <Show when={error()}>
        <div class="mb-4">
          <Alert tone="error">{error()}</Alert>
        </div>
      </Show>

      <Card class="mb-4 p-5">
        <h2 class="mb-3 text-sm font-bold text-zinc-800 dark:text-zinc-100">{editing() ? `Edit ${editing()}` : 'New geofence'}</h2>
        <form onSubmit={submit} class="flex flex-col gap-4">
          <div class="flex gap-2">
            <input
              value={search()}
              onInput={(e) => setSearch(e.currentTarget.value)}
              placeholder="Search a place (OpenStreetMap)"
              class="flex-1 rounded-xl border border-zinc-300 dark:border-white/10 bg-zinc-900/[0.04] dark:bg-white/[0.04] px-3 py-2 text-sm text-zinc-800 dark:text-zinc-100 placeholder:text-zinc-400 dark:placeholder:text-zinc-600 focus:border-[#e82127]/60 focus:outline-none focus:ring-2 focus:ring-[#e82127]/20"
            />
            <Button onClick={doSearch} variant="secondary">
              <Icon d={I.search} class="h-4 w-4" /> Search
            </Button>
          </div>
          <div class="overflow-hidden rounded-xl ring-1 ring-zinc-900/10 dark:ring-white/10">
            <div ref={(el) => (mapEl = el)} class="h-64 w-full" />
          </div>
          <FormField label="Name" value={form().name} onInput={(v) => patch({ name: v })} placeholder="Home" />
          <div class="grid grid-cols-3 gap-3">
            <FormField label="Latitude" value={latText()} onInput={setLatText} />
            <FormField label="Longitude" value={lngText()} onInput={setLngText} />
            <FormField label="Radius (m)" value={radiusText()} onInput={setRadiusText} />
          </div>
          <Select
            label="Billing"
            value={billingMode()}
            onChange={(v) => setBillingMode(v as 'none' | 'per_kwh' | 'per_minute')}
            options={[
              { value: 'none', label: 'No billing' },
              { value: 'per_kwh', label: 'Per kWh' },
              { value: 'per_minute', label: 'Per minute' },
            ]}
          />
          <Show when={billingMode() !== 'none'}>
            <div class="grid grid-cols-2 gap-3">
              <FormField
                label={billingMode() === 'per_kwh' ? 'Cost per kWh' : 'Cost per minute'}
                value={rate()}
                onInput={setRate}
              />
              <FormField label="Session fee" value={fee()} onInput={setFee} />
            </div>
          </Show>
          <div class="flex gap-2">
            <Button type="submit" loading={busy()}>
              {editing() ? 'Update' : 'Create'}
            </Button>
            <Show when={editing()}>
              <Button onClick={startCreate} variant="ghost">
                Cancel
              </Button>
            </Show>
          </div>
        </form>
      </Card>

      <Show when={list.loading}>
        <Spinner />
      </Show>
      <div class="grid gap-3 md:grid-cols-2">
        <For each={fences()}>
          {(g) => (
            <Card class="p-4">
              <div class="mb-1 flex items-center justify-between gap-2">
                <h2 class="truncate text-sm font-bold text-zinc-800 dark:text-zinc-100">{g.name}</h2>
                <Pill tone={g.billing ? 'green' : 'gray'}>{billingLabel(g)}</Pill>
              </div>
              <p class="mb-3 font-mono text-xs text-zinc-900 dark:text-zinc-500">
                {g.latitude.toFixed(4)}, {g.longitude.toFixed(4)} · {g.radius_meters} m
              </p>
              <div class="flex gap-2">
                <Button onClick={() => startEdit(g)} variant="secondary" size="sm">
                  Edit
                </Button>
                <Button onClick={() => remove(g.name)} variant="danger" size="sm">
                  {confirmDelete() === g.name ? 'Confirm delete?' : 'Delete'}
                </Button>
              </div>
            </Card>
          )}
        </For>
      </div>
      <Show when={!list.loading && list.error}>
        <div class="mt-3">
          <Alert tone="error">Could not load geofences.</Alert>
        </div>
      </Show>
      <Show when={!list.loading && !list.error && fences().length === 0}>
        <div class="mt-3">
          <EmptyState title="No geofences yet" hint="Create one above — home, work, or your favorite charger." />
        </div>
      </Show>
    </div>
  )
}
