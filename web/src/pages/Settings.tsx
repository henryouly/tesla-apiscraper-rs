import { A } from '@solidjs/router'
import { For, Show, createEffect, createResource, createSignal } from 'solid-js'
import { ApiError, api, type GlobalSettings } from '../lib/api'
import { useTheme, type Theme } from '../lib/theme'
import { useUnits } from '../lib/units'
import { Button, Card, FormField, Select, Spinner } from '../components/ui'

export function Settings() {
  const { setTheme } = useTheme()
  const { refresh: refreshUnits } = useUnits()
  const [data, { refetch }] = createResource(() => api.settings())
  const [vehicles] = createResource(() => api.vehicles())
  const [draft, setDraft] = createSignal<GlobalSettings | null>(null)
  const [error, setError] = createSignal<string | null>(null)
  const [saved, setSaved] = createSignal(false)
  const [busy, setBusy] = createSignal(false)

  createEffect(() => {
    const d = data()
    if (d && !draft()) setDraft({ ...d.settings.global })
  })

  const patch = (p: Partial<GlobalSettings>) =>
    setDraft((d) => (d ? { ...d, ...p } : d))

  const submit = async (e: Event) => {
    e.preventDefault()
    const d = draft()
    if (!d) return
    setBusy(true)
    setError(null)
    setSaved(false)
    try {
      const savedSettings = await api.saveGlobalSettings(d)
      setDraft({ ...savedSettings })
      setTheme(savedSettings.theme as Theme)
      // Refresh the shared copy so cards pick up unit_length without reload.
      refreshUnits()
      setSaved(true)
      refetch()
    } catch (err) {
      setError(err instanceof ApiError ? err.body : 'Save failed')
    } finally {
      setBusy(false)
    }
  }

  return (
    <div class="mx-auto max-w-md">
      <h1 class="mb-4 text-xl font-bold">Settings</h1>
      <Show when={data.loading}>
        <Spinner />
      </Show>
      <Show when={data.error}>
        <p class="text-sm text-red-600">Could not load settings.</p>
      </Show>
      <Show when={draft()}>
        <Card>
          <form onSubmit={submit} class="flex flex-col gap-3">
            <Select
              label="Unit of length"
              value={draft()!.unit_length}
              onChange={(v) => patch({ unit_length: v })}
              options={[{ value: 'km' }, { value: 'mi' }]}
            />
            <Select
              label="Unit of temperature"
              value={draft()!.unit_temperature}
              onChange={(v) => patch({ unit_temperature: v })}
              options={[{ value: 'C', label: '°C' }, { value: 'F', label: '°F' }]}
            />
            <Select
              label="Unit of pressure"
              value={draft()!.unit_pressure}
              onChange={(v) => patch({ unit_pressure: v })}
              options={[{ value: 'bar' }, { value: 'psi' }]}
            />
            <Select
              label="Preferred range"
              value={draft()!.preferred_range}
              onChange={(v) => patch({ preferred_range: v })}
              options={[{ value: 'rated' }, { value: 'ideal' }]}
            />
            <FormField
              label="Language"
              value={draft()!.language}
              onInput={(v) => patch({ language: v })}
            />
            <p class="-mt-2 text-xs text-gray-500">
              Stored for future use; the UI is English-only for now.
            </p>
            <Select
              label="Theme"
              value={draft()!.theme}
              onChange={(v) => {
                // Local preference: preview immediately, persist on save.
                patch({ theme: v })
                setTheme(v as Theme)
              }}
              options={[{ value: 'light' }, { value: 'dark' }, { value: 'system' }]}
            />
            <p class="text-xs text-gray-500">
              Length units apply to car cards; temperature and pressure display
              follow in a later update. Grafana URL is environment-controlled,
              not stored here.
            </p>
            <Show when={error()}>
              <p class="text-sm text-red-600">{error()}</p>
            </Show>
            <Show when={saved()}>
              <p class="text-sm text-green-600">Saved.</p>
            </Show>
            <Button type="submit" disabled={busy()}>
              {busy() ? 'Saving…' : 'Save settings'}
            </Button>
          </form>
        </Card>
      </Show>
      <div class="mt-4">
        <h2 class="mb-2 text-sm font-bold text-gray-500">Per-car settings</h2>
        <div class="flex flex-col gap-1">
          <For each={vehicles()?.vehicles ?? []}>
            {(v) => (
              <A
                href={`/settings/car/${encodeURIComponent(v.vin)}`}
                class="text-sm text-blue-600 dark:text-blue-400"
              >
                {v.display_name || v.vin}
              </A>
            )}
          </For>
        </div>
      </div>
    </div>
  )
}
