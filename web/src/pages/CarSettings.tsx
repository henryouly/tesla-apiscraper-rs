import { A, useParams } from '@solidjs/router'
import { For, Show, createEffect, createResource, createSignal } from 'solid-js'
import { ApiError, api, type CarSettings as CarSettingsData } from '../lib/api'
import { parseNumber } from '../lib/number'
import { Button, Card, Check, FormField, Spinner } from '../components/ui'

const DEFAULTS: CarSettingsData = {
  suspend_after_idle_minutes: 21,
  suspend_minimum_minutes: 15,
  require_unlocked_for_wake: false,
  free_supercharging: false,
  use_streaming_api: false,
  enabled: true,
  lfp_battery: false,
}

export function CarSettings() {
  const params = useParams()
  const vin = () => decodeURIComponent(params.id ?? '')
  const [vehicles] = createResource(() => api.vehicles())
  const [data, { refetch }] = createResource(() => api.settings())
  const [draft, setDraft] = createSignal<CarSettingsData | null>(null)
  const [idleText, setIdleText] = createSignal('')
  const [minText, setMinText] = createSignal('')
  const [error, setError] = createSignal<string | null>(null)
  const [saved, setSaved] = createSignal(false)
  const [busy, setBusy] = createSignal(false)

  // Seed once per VIN from stored settings or defaults.
  createEffect(() => {
    const d = data()
    if (!d || draft()) return
    const stored = d.settings.cars[vin()] ?? DEFAULTS
    setDraft({ ...stored })
    setIdleText(String(stored.suspend_after_idle_minutes))
    setMinText(String(stored.suspend_minimum_minutes))
  })

  const patch = (p: Partial<CarSettingsData>) =>
    setDraft((d) => (d ? { ...d, ...p } : d))

  const nameOf = () =>
    vehicles()?.vehicles.find((v) => v.vin === vin())?.display_name || vin()

  const submit = async (e: Event) => {
    e.preventDefault()
    const d = draft()
    if (!d) return
    const idle = parseNumber(idleText())
    const min = parseNumber(minText())
    if (idle == null || min == null) {
      setError('Suspend timers must be numbers')
      return
    }
    setBusy(true)
    setError(null)
    setSaved(false)
    try {
      const savedSettings = await api.saveCarSettings(vin(), {
        ...d,
        suspend_after_idle_minutes: idle,
        suspend_minimum_minutes: min,
      })
      setDraft({ ...savedSettings })
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
      <A href="/settings" class="mb-2 inline-block text-sm text-gray-500">
        ← Settings
      </A>
      <h1 class="mb-4 text-xl font-bold">{nameOf()}</h1>
      <Show when={data.loading}>
        <Spinner />
      </Show>
      <Show when={data.error}>
        <p class="text-sm text-red-600">Could not load settings.</p>
      </Show>
      <Show when={draft()}>
        <Card>
          <form onSubmit={submit} class="flex flex-col gap-3">
            <div class="grid grid-cols-2 gap-3">
              <FormField
                label="Suspend after idle (min)"
                value={idleText()}
                onInput={setIdleText}
              />
              <FormField
                label="Suspend minimum (min)"
                value={minText()}
                onInput={setMinText}
              />
            </div>
            <Check
              label="Require unlocked for wake"
              checked={draft()!.require_unlocked_for_wake}
              onChange={(v) => patch({ require_unlocked_for_wake: v })}
            />
            <Check
              label="Free supercharging"
              checked={draft()!.free_supercharging}
              onChange={(v) => patch({ free_supercharging: v })}
            />
            <Check
              label="Use streaming API"
              checked={draft()!.use_streaming_api}
              onChange={(v) => patch({ use_streaming_api: v })}
            />
            <Check
              label="Enabled"
              checked={draft()!.enabled}
              onChange={(v) => patch({ enabled: v })}
            />
            <Check
              label="LFP battery"
              checked={draft()!.lfp_battery}
              onChange={(v) => patch({ lfp_battery: v })}
            />
            <Show when={error()}>
              <p class="text-sm text-red-600">{error()}</p>
            </Show>
            <Show when={saved()}>
              <p class="text-sm text-green-600">Saved.</p>
            </Show>
            <Button type="submit" disabled={busy()}>
              {busy() ? 'Saving…' : 'Save car settings'}
            </Button>
          </form>
        </Card>
      </Show>
      <Show when={!data.loading && !data.error}>
        <div class="mt-4">
          <h2 class="mb-2 text-sm font-bold text-gray-500">All cars</h2>
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
      </Show>
    </div>
  )
}
