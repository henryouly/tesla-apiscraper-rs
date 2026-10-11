import { Show, createMemo, createResource } from 'solid-js'
import { useDashboardScope } from '../components/DashboardsLayout'
import { BigStat, DataTable, type DataColumn } from '../components/data'
import { Alert, Card, EmptyState, Spinner } from '../components/ui'
import { fetchUpdates, type SoftwareUpdate } from '../lib/dashboards'

interface UpdateRow extends Record<string, unknown> {
  t: number
  version_before: string | null
  version_after: string | null
  install_start: string | null
  install_end: string | null
  status: string | null
}

function toRow(u: SoftwareUpdate): UpdateRow {
  return {
    t: u.t,
    version_before: u.version_before,
    version_after: u.version_after,
    install_start: u.install_start,
    install_end: u.install_end,
    status: u.status,
  }
}

const COLUMNS: Array<DataColumn<UpdateRow>> = [
  { key: 'version_before', label: 'From' },
  { key: 'version_after', label: 'To' },
  { key: 'status', label: 'Status' },
  { key: 'install_start', label: 'Install Start' },
  { key: 'install_end', label: 'Install End' },
]

export function Updates() {
  const scope = useDashboardScope()
  const range = () => scope.range()
  const vin = () => scope.vin()

  const [updates] = createResource(
    () => {
      const v = vin()
      if (v == null) return false
      return { vin: v, range: range() }
    },
    (s) => fetchUpdates({ vin: s.vin, from: s.range.from, to: s.range.to }),
  )

  const rows = createMemo(() => (updates()?.updates ?? []).map(toRow))
  const isEmpty = () => (updates()?.updates.length ?? 0) === 0

  return (
    <div>
      <Show when={vin()}>
        <Show when={updates.loading && !updates()}>
          <Spinner />
        </Show>
        <Show when={updates.error}>
          <div class="mb-4">
            <Alert tone="error">
              {updates.error instanceof Error ? updates.error.message : 'Failed to load updates data.'}
            </Alert>
          </div>
        </Show>
        <Show when={!updates.loading && !updates.error && updates() && isEmpty()}>
          <EmptyState
            title="No updates for this period"
            hint="Try a wider time range, or check back once the car installs a software update."
          />
        </Show>
        <Show when={!updates.error && updates() && !isEmpty()}>
          <div class="mb-4 grid gap-4 sm:grid-cols-2">
            <BigStat label="Software Updates" value={String(updates()?.count ?? 0)} sub={updates()?.vin} />
          </div>
          <Card class="mb-4 p-5">
            <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">Updates</h2>
            <DataTable columns={COLUMNS} rows={rows()} />
          </Card>
        </Show>
      </Show>
    </div>
  )
}
