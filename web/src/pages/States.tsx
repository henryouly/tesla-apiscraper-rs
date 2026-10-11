import { Show, createResource } from 'solid-js'
import { useDashboardScope } from '../components/DashboardsLayout'
import { StateTimeline } from '../components/StateTimeline'
import { BigStat } from '../components/data'
import { Alert, Card, EmptyState, Spinner } from '../components/ui'
import { fetchStates } from '../lib/dashboards'

export function States() {
  const scope = useDashboardScope()
  const range = () => scope.range()
  const vin = () => scope.vin()

  const [states] = createResource(
    () => {
      const v = vin()
      if (v == null) return false
      return { vin: v, range: range() }
    },
    (s) => fetchStates({ vin: s.vin, from: s.range.from, to: s.range.to }),
  )

  const isEmpty = () => states()?.current == null && (states()?.timeline.length ?? 0) === 0

  const domain = () => ({ from: Date.parse(range().from), to: Date.parse(range().to) })

  return (
    <div>
      <Show when={vin()}>
        <Show when={states.loading && !states()}>
          <Spinner />
        </Show>
        <Show when={states.error}>
          <div class="mb-4">
            <Alert tone="error">
              {states.error instanceof Error ? states.error.message : 'Failed to load states data.'}
            </Alert>
          </div>
        </Show>
        <Show when={!states.loading && !states.error && states() && isEmpty()}>
          <EmptyState
            title="No data for this period"
            hint="Try a different vehicle, or check back once the car reports telemetry."
          />
        </Show>
        <Show when={!states.error && states() && !isEmpty()}>
          <div class="mb-4 grid gap-4 sm:grid-cols-2">
            <BigStat label="Current State" value={states()?.current ?? '—'} sub={states()?.vin} />
            <BigStat label="State Changes" value={String(states()?.changes ?? 0)} />
          </div>
          <Card class="mb-4 p-5">
            <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">States</h2>
            <StateTimeline data={states()?.timeline ?? []} from={domain().from} to={domain().to} />
          </Card>
        </Show>
      </Show>
    </div>
  )
}
