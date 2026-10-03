import { useParams } from '@solidjs/router'
import { Show, createResource, createSignal } from 'solid-js'
import { ApiError, api, previewCost } from '../lib/api'
import { parseNumber } from '../lib/number'
import { Alert, Button, Card, FormField, PageHeader, Select, Spinner, Stat } from '../components/ui'

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
    <div>
      <PageHeader eyebrow="Charging" title="Charge cost" hint={id()} />
      <div class="mx-auto max-w-xl">
        <Show when={session.loading}>
          <Spinner />
        </Show>
        <Show when={loadError()}>
          <Alert tone="error">{loadError()}</Alert>
        </Show>
        <Show when={session()}>
          <div class="mb-4 grid grid-cols-3 gap-2">
            <Stat label="Energy added" value={energyWh() ? `${(energyWh() / 1000).toFixed(1)} kWh` : '—'} />
            <Stat label="Duration" value={durationSec() ? `${Math.round(durationSec() / 60)} min` : '—'} />
            <Stat label="Current cost" value={num(session()?.cost) != null ? String(num(session()?.cost)) : '—'} />
          </div>
          <Card class="p-5">
            <form onSubmit={submit} class="flex flex-col gap-4">
              <Select
                label="Billing mode"
                value={mode()}
                onChange={(v) => setMode(v as 'per_kwh' | 'per_minute')}
                options={[
                  { value: 'per_kwh', label: 'Per kWh' },
                  { value: 'per_minute', label: 'Per minute' },
                ]}
              />
              <div class="grid grid-cols-2 gap-3">
                <FormField
                  label={mode() === 'per_kwh' ? 'Cost per kWh' : 'Cost per minute'}
                  value={rate()}
                  onInput={setRate}
                />
                <FormField label="Session fee" value={fee()} onInput={setFee} />
              </div>
              <div class="rounded-xl border border-zinc-200 dark:border-white/[0.07] bg-zinc-900/[0.03] dark:bg-white/[0.03] px-3 py-2.5 text-sm text-zinc-600 dark:text-zinc-300">
                Preview: <strong class="text-zinc-900 dark:text-zinc-50">{preview() != null ? preview()!.toFixed(2) : '—'}</strong>
              </div>
              <Show when={error()}>
                <Alert tone="error">{error()}</Alert>
              </Show>
              <Show when={saved() != null}>
                <Alert tone="success">Saved cost {saved()!.toFixed(2)}</Alert>
              </Show>
              <Button type="submit" loading={busy()}>
                Save cost
              </Button>
            </form>
          </Card>
        </Show>
      </div>
    </div>
  )
}
