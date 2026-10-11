import { Show, createMemo, createResource } from 'solid-js'
import { useDashboardScope } from '../components/DashboardsLayout'
import { BarGauge } from '../components/BarGauge'
import { BigStat, DataTable, type DataColumn } from '../components/data'
import { Alert, Card, EmptyState, Spinner } from '../components/ui'
import { fetchLocations, type DistanceUnit, type VisitedRow } from '../lib/dashboards'
import { useUnits } from '../lib/units'

interface VisitTableRow extends Record<string, unknown> {
  t: number
  address: string | null
  geofence: string | null
  distance: number | null
}

function toRow(r: VisitedRow): VisitTableRow {
  return { t: r.t, address: r.address, geofence: r.geofence, distance: r.distance }
}

export function Locations() {
  const units = useUnits()
  const scope = useDashboardScope()
  const range = () => scope.range()
  // Distance unit always follows the global setting (Settings page);
  // there is no page-local override.
  const unit = (): DistanceUnit => (units.unitLength() === 'mi' ? 'mi' : 'km')
  const vin = () => scope.vin()

  const [locations] = createResource(
    () => {
      const v = vin()
      if (v == null) return false
      return { vin: v, range: range(), unit: unit() }
    },
    (s) => fetchLocations({ vin: s.vin, from: s.range.from, to: s.range.to, unit: s.unit }),
  )

  const rows = createMemo(() => (locations()?.last_visited ?? []).map(toRow))
  const isEmpty = () => (locations()?.last_visited.length ?? 0) === 0 && (locations()?.top.length ?? 0) === 0

  const distanceText = (v: number | null) =>
    v == null ? '—' : `${Math.round(v * 10) / 10} ${unit()}`

  const columns = createMemo((): Array<DataColumn<VisitTableRow>> => [
    { key: 't', label: 'Time', render: (row) => new Date(row.t).toLocaleString() },
    { key: 'address', label: 'Address' },
    { key: 'geofence', label: 'Geofence' },
    {
      key: 'distance',
      label: `Distance (${unit()})`,
      align: 'right',
      render: (row) => distanceText(row.distance),
    },
  ])

  return (
    <div>
      <Show when={vin()}>
        <Show when={locations.loading && !locations()}>
          <Spinner />
        </Show>
        <Show when={locations.error}>
          <div class="mb-4">
            <Alert tone="error">
              {locations.error instanceof Error ? locations.error.message : 'Failed to load locations data.'}
            </Alert>
          </div>
        </Show>
        <Show when={!locations.loading && !locations.error && locations() && isEmpty()}>
          <EmptyState
            title="No drives for this period"
            hint="Try a wider time range, or check back once the car completes a drive."
          />
        </Show>
        <Show when={!locations.error && locations() && !isEmpty()}>
          <div class="mb-4 grid gap-4 sm:grid-cols-2">
            <BigStat label="Drives Logged" value={String(locations()?.drives_logged ?? 0)} sub={locations()?.vin} />
          </div>
          <Card class="mb-4 p-5">
            <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">Top Destinations</h2>
            <BarGauge items={(locations()?.top ?? []).map((d) => ({ label: d.place, value: d.visits }))} unit="visits" />
          </Card>
          <Card class="mb-4 p-5">
            <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">Last Visited</h2>
            <DataTable columns={columns()} rows={rows()} />
          </Card>
        </Show>
      </Show>
    </div>
  )
}
