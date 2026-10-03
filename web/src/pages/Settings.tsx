import { A } from '@solidjs/router'
import { For, Show, createEffect, createResource, createSignal } from 'solid-js'
import { ApiError, api, type GlobalSettings } from '../lib/api'
import { useTheme, type Theme } from '../lib/theme'
import { useUnits } from '../lib/units'
import { Alert, Button, Card, FormField, I, Icon, PageHeader, Select, Spinner } from '../components/ui'

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

  const patch = (p: Partial<GlobalSettings>) => setDraft((d) => (d ? { ...d, ...p } : d))

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
    <div>
      <PageHeader eyebrow="Preferences" title="Settings" hint="Units, display language and theme. Length units apply to car cards immediately." />
      <div class="mx-auto max-w-xl">
        <Show when={data.loading}>
          <Spinner />
        </Show>
        <Show when={data.error}>
          <Alert tone="error">Could not load settings.</Alert>
        </Show>
        <Show when={draft()}>
          <Card class="p-5">
            <form onSubmit={submit} class="flex flex-col gap-4">
              <div class="grid gap-4 sm:grid-cols-2">
                <Select
                  label="Unit of length"
                  value={draft()!.unit_length}
                  onChange={(v) => patch({ unit_length: v })}
                  options={[{ value: 'km' }, { value: 'mi' }]}
                />
                <Select
                  label="Theme"
                  value={draft()!.theme}
                  onChange={(v) => {
                    patch({ theme: v })
                    setTheme(v as Theme)
                  }}
                  options={[{ value: 'light' }, { value: 'dark' }, { value: 'system' }]}
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
              </div>
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
                hint="Stored for future use; the UI is English-only for now."
              />
              <Show when={error()}>
                <Alert tone="error">{error()}</Alert>
              </Show>
              <Show when={saved()}>
                <Alert tone="success">Saved.</Alert>
              </Show>
              <Button type="submit" loading={busy()}>
                Save settings
              </Button>
            </form>
          </Card>
        </Show>
        <Card class="mt-4 p-5">
          <h2 class="mb-1 text-sm font-bold text-zinc-100">Per-car settings</h2>
          <p class="mb-3 text-xs text-zinc-500">Suspend timers, streaming API, battery type.</p>
          <div class="flex flex-col gap-1">
            <For each={vehicles()?.vehicles ?? []}>
              {(v) => (
                <A
                  href={`/settings/car/${encodeURIComponent(v.vin)}`}
                  class="flex items-center justify-between rounded-xl px-3 py-2 text-sm text-zinc-200 transition-colors hover:bg-white/[0.05]"
                >
                  <span class="truncate">{v.display_name || v.vin}</span>
                  <Icon d={I.back} class="h-4 w-4 rotate-180 text-zinc-600" />
                </A>
              )}
            </For>
          </div>
        </Card>
      </div>
    </div>
  )
}
