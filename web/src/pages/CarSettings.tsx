import { A, useParams } from '@solidjs/router'
import { For, Show, createEffect, createResource, createSignal } from 'solid-js'
import { ApiError, api, type CarSettings as CarSettingsData } from '../lib/api'
import { parseNonNegativeInt } from '../lib/number'
import { Alert, BackLink, Button, Card, Check, FormField, PageHeader, Spinner } from '../components/ui'

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
  const [seededFor, setSeededFor] = createSignal<string | null>(null)
  const [idleText, setIdleText] = createSignal('')
  const [minText, setMinText] = createSignal('')
  const [error, setError] = createSignal<string | null>(null)
  const [saved, setSaved] = createSignal(false)
  const [busy, setBusy] = createSignal(false)

  createEffect(() => {
    const d = data()
    if (!d || seededFor() === vin()) return
    const stored = d.settings.cars[vin()] ?? DEFAULTS
    setDraft({ ...stored })
    setIdleText(String(stored.suspend_after_idle_minutes))
    setMinText(String(stored.suspend_minimum_minutes))
    setSeededFor(vin())
  })

  const patch = (p: Partial<CarSettingsData>) => setDraft((d) => (d ? { ...d, ...p } : d))

  const nameOf = () => vehicles()?.vehicles.find((v) => v.vin === vin())?.display_name || 'Car settings'

  const submit = async (e: Event) => {
    e.preventDefault()
    const d = draft()
    if (!d) return
    const idle = parseNonNegativeInt(idleText())
    const min = parseNonNegativeInt(minText())
    if (idle == null || min == null) {
      setError('Suspend timers must be whole non-negative numbers')
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
    <div>
      <div class="mx-auto max-w-xl">
        <BackLink href="/settings">Settings</BackLink>
        <PageHeader eyebrow="Vehicle" title={nameOf()} hint={vin()} />
        <Show when={data.loading}>
          <Spinner />
        </Show>
        <Show when={data.error}>
          <Alert tone="error">Could not load settings.</Alert>
        </Show>
        <Show when={draft()}>
          <Card class="p-5">
            <form onSubmit={submit} class="flex flex-col gap-3">
              <div class="grid grid-cols-2 gap-3">
                <FormField label="Suspend after idle (min)" value={idleText()} onInput={setIdleText} />
                <FormField label="Suspend minimum (min)" value={minText()} onInput={setMinText} />
              </div>
              <Check label="Enabled" hint="Log telemetry for this car" checked={draft()!.enabled} onChange={(v) => patch({ enabled: v })} />
              <Check label="Use streaming API" hint="Higher-resolution drive data when available" checked={draft()!.use_streaming_api} onChange={(v) => patch({ use_streaming_api: v })} />
              <Check label="Require unlocked for wake" hint="Only wake the car when unlocked" checked={draft()!.require_unlocked_for_wake} onChange={(v) => patch({ require_unlocked_for_wake: v })} />
              <Check label="Free supercharging" hint="Stored for future cost math" checked={draft()!.free_supercharging} onChange={(v) => patch({ free_supercharging: v })} />
              <Check label="LFP battery" hint="Stored for future range math" checked={draft()!.lfp_battery} onChange={(v) => patch({ lfp_battery: v })} />
              <Show when={error()}>
                <Alert tone="error">{error()}</Alert>
              </Show>
              <Show when={saved()}>
                <Alert tone="success">Saved.</Alert>
              </Show>
              <Button type="submit" loading={busy()}>
                Save car settings
              </Button>
            </form>
          </Card>
        </Show>
        <Show when={!data.loading && !data.error}>
          <Card class="mt-4 p-5">
            <h2 class="mb-2 text-sm font-bold text-zinc-800 dark:text-zinc-100">All cars</h2>
            <div class="flex flex-col gap-1">
              <For each={vehicles()?.vehicles ?? []}>
                {(v) => (
                  <A
                    href={`/settings/car/${encodeURIComponent(v.vin)}`}
                    class="rounded-xl px-3 py-2 text-sm text-zinc-600 dark:text-zinc-300 transition-colors hover:bg-zinc-900/[0.05] dark:hover:bg-white/[0.05] hover:text-zinc-900 dark:hover:text-white"
                  >
                    {v.display_name || v.vin}
                  </A>
                )}
              </For>
            </div>
          </Card>
        </Show>
      </div>
    </div>
  )
}
