import { Show, createMemo, createResource } from 'solid-js'
import { useDashboardScope } from '../components/DashboardsLayout'
import { BigStat, DataTable, type DataColumn } from '../components/data'
import { Alert, Button, Card, EmptyState, Pill, Spinner } from '../components/ui'
import { defaultRange, fetchUpdates, type SoftwareUpdate } from '../lib/dashboards'
import {
  computeInstallDurationSec,
  formatUpdateDuration,
  formatUpdateTimestamp,
  updateStatusBadge,
} from '../lib/updates'

interface UpdateRow extends Record<string, unknown> {
  t: number
  version_before: string | null
  version_after: string | null
  install_start: string | null
  install_end: string | null
  duration_s: number | null
  status: string | null
  abandoned: boolean | null
}

function toRow(u: SoftwareUpdate): UpdateRow {
  return {
    t: u.t,
    version_before: u.version_before,
    version_after: u.version_after,
    install_start: u.install_start,
    install_end: u.install_end,
    duration_s: computeInstallDurationSec(u.install_start, u.install_end),
    status: u.status,
    abandoned: u.abandoned ?? null,
  }
}

const COLUMNS: Array<DataColumn<UpdateRow>> = [
  { key: 'version_before', label: 'From' },
  { key: 'version_after', label: 'To' },
  {
    key: 'status',
    label: 'Status',
    render: (row) => {
      const badge = updateStatusBadge(row.status, row.abandoned)
      return (
        <Pill tone={badge.tone} pulse={badge.label === 'installing'}>
          {badge.label}
        </Pill>
      )
    },
  },
  {
    key: 'duration_s',
    label: 'Duration',
    align: 'right',
    render: (row) => <span class="tabular-nums">{formatUpdateDuration(row.duration_s)}</span>,
  },
  {
    key: 'install_start',
    label: 'Install Start',
    render: (row) => <span class="whitespace-nowrap">{formatUpdateTimestamp(row.install_start)}</span>,
  },
  {
    key: 'install_end',
    label: 'Install End',
    render: (row) => <span class="whitespace-nowrap">{formatUpdateTimestamp(row.install_end)}</span>,
  },
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

  const safeUpdates = () => (updates.error ? undefined : updates())
  const rows = createMemo(() => (safeUpdates()?.updates ?? []).map(toRow))
  const isEmpty = () => (safeUpdates()?.updates.length ?? 0) === 0

  const latestVersion = createMemo(() => {
    for (const r of rows()) {
      if (r.version_after) return r.version_after
      if (r.version_before) return r.version_before
    }
    return '—'
  })

  const avgDurationText = createMemo(() => {
    const valid = rows()
      .map((r) => r.duration_s)
      .filter((d): d is number => d != null)
    if (valid.length === 0) return '—'
    const avg = valid.reduce((acc, v) => acc + v, 0) / valid.length
    return formatUpdateDuration(avg)
  })

  const expandToOneYear = () => {
    scope.setRange(defaultRange(365))
  }

  return (
    <div>
      <Show when={vin()}>
        <Show when={updates.loading && !safeUpdates()}>
          <Spinner />
        </Show>
        <Show when={updates.error}>
          <div class="mb-4">
            <Alert tone="error">
              {updates.error instanceof Error ? updates.error.message : 'Failed to load updates data.'}
            </Alert>
          </div>
        </Show>
        <Show when={!updates.loading && !updates.error && safeUpdates() && isEmpty()}>
          <EmptyState
            title="No updates for this period"
            hint="Software updates happen every few weeks. Try expanding the window to the last year."
            action={
              <Button variant="secondary" size="sm" onClick={expandToOneYear}>
                Show last 1 year
              </Button>
            }
          />
        </Show>
        <Show when={!updates.error && safeUpdates() && !isEmpty()}>
          <div class="mb-4 grid gap-4 sm:grid-cols-3">
            <BigStat label="Software Updates" value={String(safeUpdates()?.count ?? 0)} sub={safeUpdates()?.vin} />
            <BigStat label="Latest Version" value={latestVersion()} />
            <BigStat label="Avg Install Time" value={avgDurationText()} />
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
