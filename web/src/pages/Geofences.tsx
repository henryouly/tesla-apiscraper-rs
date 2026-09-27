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
import { Button, Card, FormField, Spinner } from '../components/ui'

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
      color: '#2563eb',
      fillColor: '#2563eb',
      fillOpacity: 0.9,
    }).addTo(map)
    circle = L.circle([form().latitude, form().longitude], {
      radius: form().radius_meters,
      color: '#2563eb',
      weight: 1,
      fillOpacity: 0.1,
    }).addTo(map)
    map.on('click', (e: L.LeafletMouseEvent) => {
      patch({ latitude: +e.latlng.lat.toFixed(6), longitude: +e.latlng.lng.toFixed(6) })
    })
  })

  // Map follows the form (marker drag edits the form via marker events is
  // skipped; click-to-place plus numeric fields keep one direction simple).
  createEffect(() => {
    const f = form()
    marker?.setLatLng([f.latitude, f.longitude])
    circle?.setLatLng([f.latitude, f.longitude])
    circle?.setRadius(f.radius_meters > 0 ? f.radius_meters : 0)
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
    if (g.billing) {
      setBillingMode(g.billing.type)
      setRate(String(g.billing.cost_per_unit))
      setFee(String(g.billing.session_fee))
    } else {
      setBillingMode('none')
    }
    map?.setView([g.latitude, g.longitude], 14)
    window.scrollTo({ top: 0, behavior: 'smooth' })
  }

  const startCreate = () => {
    setForm(emptyForm())
    setEditing(null)
    setBillingMode('none')
    setRate('0.3')
    setFee('0')
    setError(null)
  }

  const submit = async (e: Event) => {
    e.preventDefault()
    setBusy(true)
    setError(null)
    const f = form()
    const payload: Geofence = {
      ...f,
      radius_meters: +f.radius_meters || 0,
      billing:
        billingMode() === 'none'
          ? null
          : {
              type: billingMode() as 'per_kwh' | 'per_minute',
              cost_per_unit: +rate() || 0,
              session_fee: +fee() || 0,
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
      <h1 class="mb-4 text-xl font-bold">Geofences</h1>
      <Show when={error()}>
        <p class="mb-3 text-sm text-red-600">{error()}</p>
      </Show>

      <Card class="mb-4">
        <h2 class="mb-2 font-bold">{editing() ? `Edit ${editing()}` : 'New geofence'}</h2>
        <form onSubmit={submit} class="flex flex-col gap-3">
          <div class="flex gap-2">
            <input
              value={search()}
              onInput={(e) => setSearch(e.currentTarget.value)}
              placeholder="Search a place (OpenStreetMap)"
              class="flex-1 rounded border border-gray-300 px-3 py-2 text-sm dark:border-gray-700 dark:bg-gray-800"
            />
            <Button onClick={doSearch} variant="ghost">
              Search
            </Button>
          </div>
          <div ref={(el) => (mapEl = el)} class="h-64 w-full rounded border border-gray-300 dark:border-gray-700" />
          <p class="text-xs text-gray-500">Click the map to place the center.</p>
          <FormField
            label="Name"
            value={form().name}
            onInput={(v) => patch({ name: v })}
            placeholder="Home"
          />
          <div class="grid grid-cols-3 gap-3">
            <FormField
              label="Latitude"
              value={String(form().latitude)}
              onInput={(v) => patch({ latitude: +v || 0 })}
            />
            <FormField
              label="Longitude"
              value={String(form().longitude)}
              onInput={(v) => patch({ longitude: +v || 0 })}
            />
            <FormField
              label="Radius (m)"
              value={String(form().radius_meters)}
              onInput={(v) => patch({ radius_meters: +v || 0 })}
            />
          </div>
          <label class="block">
            <span class="mb-1 block text-sm font-medium">Billing</span>
            <select
              value={billingMode()}
              onChange={(e) =>
                setBillingMode(e.currentTarget.value as 'none' | 'per_kwh' | 'per_minute')
              }
              class="w-full rounded border border-gray-300 bg-white px-3 py-2 text-sm dark:border-gray-700 dark:bg-gray-800"
            >
              <option value="none">No billing</option>
              <option value="per_kwh">Per kWh</option>
              <option value="per_minute">Per minute</option>
            </select>
          </label>
          <Show when={billingMode() !== 'none'}>
            <div class="grid grid-cols-2 gap-3">
              <FormField
                label={billingMode() === 'per_kwh' ? 'Cost per kWh' : 'Cost per minute'}
                value={rate()}
                onInput={setRate}
              />
              <FormField label="Session fee" value={fee()} onInput={setFee} />
            </div>
            <p class="text-xs text-gray-500">
              Billing changes apply to future charging sessions only.
            </p>
          </Show>
          <div class="flex gap-2">
            <Button type="submit" disabled={busy()}>
              {busy() ? 'Saving…' : editing() ? 'Update' : 'Create'}
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
      <div class="grid gap-4 md:grid-cols-2">
        <For each={fences()}>
          {(g) => (
            <Card>
              <div class="mb-1 flex items-center justify-between">
                <h2 class="font-bold">{g.name}</h2>
                <span class="text-xs text-gray-500">{billingLabel(g)}</span>
              </div>
              <p class="mb-3 text-sm text-gray-600 dark:text-gray-400">
                {g.latitude.toFixed(4)}, {g.longitude.toFixed(4)} · {g.radius_meters} m
              </p>
              <div class="flex gap-2">
                <Button onClick={() => startEdit(g)} variant="ghost">
                  Edit
                </Button>
                <Button onClick={() => remove(g.name)} variant="ghost">
                  {confirmDelete() === g.name ? 'Confirm delete?' : 'Delete'}
                </Button>
              </div>
            </Card>
          )}
        </For>
      </div>
      <Show when={!list.loading && fences().length === 0}>
        <p class="text-sm text-gray-500">No geofences yet — create one above.</p>
      </Show>
    </div>
  )
}
