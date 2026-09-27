import { useParams } from '@solidjs/router'
import { Show, createResource, createSignal } from 'solid-js'
import { ApiError, api, previewCost } from '../lib/api'
import { parseNumber } from '../lib/number'
import { Button, Card, FormField, Spinner } from '../components/ui'

function num(v: unknown): number | null {
  if (typeof v === 'number' && Number.isFinite(v)) return v
  return null
}

export function ChargeCost() {
  const params = useParams()
  const id = () => decodeURIComponent(params.id ?? '')
  const [session, { refetch }] = createResource(id, (vin) => api.charge(vin))
  const [mode, setMode] = createSignal<'per_kwh' | 'per_minute'>('per_kwh')
  const [rate, setRate] = createSignal('0.3')
  const [fee, setFee] = createSignal('0')
  const [error, setError] = createSignal<string | null>(null)
  const [busy, setBusy] = createSignal(false)
  const [saved, setSaved] = createSignal<number | null>(null)

  const energyWh = () => num(session()?.energy_added_wh) ?? 0
  const loadError = () => {
    const e = session.error
    if (!e) return null
    if (e instanceof ApiError && e.status === 404) return 'Session not found.'
    if (e instanceof ApiError && e.status >= 500) {
      return 'Could not load the session — the database may be unavailable.'
    }
    return 'Could not load the session.'
  }
  const durationSec = () => num(session()?.duration_seconds) ?? 0
  const preview = () => {
    const rateNum = parseNumber(rate())
    const feeNum = parseNumber(fee())
    if (rateNum == null || feeNum == null) return null
    return previewCost(mode(), energyWh(), durationSec(), rateNum, feeNum)
  }

  const submit = async (e: Event) => {
    e.preventDefault()
    const rateNum = parseNumber(rate())
    const feeNum = parseNumber(fee())
    if (rateNum == null || feeNum == null) {
      setError('Cost and fee must be numbers')
      return
    }
    setBusy(true)
    setError(null)
    setSaved(null)
    try {
      const r = await api.setChargeCost(id(), mode(), rateNum, feeNum)
      setSaved(r.cost)
      refetch()
    } catch (err) {
      setError(err instanceof ApiError ? err.body : 'Save failed')
    } finally {
      setBusy(false)
    }
  }

  return (
    <div class="mx-auto max-w-md">
      <Card>
        <h1 class="mb-1 text-xl font-bold">Charge cost</h1>
        <p class="mb-4 text-sm text-gray-500">{id()}</p>
        <Show when={session.loading}>
          <Spinner />
        </Show>
        <Show when={loadError()}>
          <p class="text-sm text-red-600">{loadError()}</p>
        </Show>
        <Show when={session()}>
          <dl class="mb-4 grid grid-cols-2 gap-x-4 gap-y-1 text-sm">
            <dt class="text-gray-500">Energy added</dt>
            <dd>{energyWh() ? `${(energyWh() / 1000).toFixed(1)} kWh` : '—'}</dd>
            <dt class="text-gray-500">Duration</dt>
            <dd>{durationSec() ? `${Math.round(durationSec() / 60)} min` : '—'}</dd>
            <dt class="text-gray-500">Current cost</dt>
            <dd>{num(session()?.cost) != null ? num(session()?.cost) : '—'}</dd>
          </dl>
          <form onSubmit={submit} class="flex flex-col gap-3">
            <label class="block">
              <span class="mb-1 block text-sm font-medium">Billing mode</span>
              <select
                value={mode()}
                onChange={(e) => setMode(e.currentTarget.value as 'per_kwh' | 'per_minute')}
                class="w-full rounded border border-gray-300 bg-white px-3 py-2 text-sm dark:border-gray-700 dark:bg-gray-800"
              >
                <option value="per_kwh">Per kWh</option>
                <option value="per_minute">Per minute</option>
              </select>
            </label>
            <div class="grid grid-cols-2 gap-3">
              <FormField
                label={mode() === 'per_kwh' ? 'Cost per kWh' : 'Cost per minute'}
                value={rate()}
                onInput={setRate}
              />
              <FormField label="Session fee" value={fee()} onInput={setFee} />
            </div>
            <p class="text-sm">
              Preview: <strong>{preview() != null ? preview()!.toFixed(2) : '—'}</strong>
            </p>
            <Show when={error()}>
              <p class="text-sm text-red-600">{error()}</p>
            </Show>
            <Show when={saved() != null}>
              <p class="text-sm text-green-600">Saved cost {saved()!.toFixed(2)}</p>
            </Show>
            <Button type="submit" disabled={busy()}>
              {busy() ? 'Saving…' : 'Save cost'}
            </Button>
          </form>
        </Show>
      </Card>
    </div>
  )
}
