import { Show, createMemo, createResource } from 'solid-js'
import { useNavigate } from '@solidjs/router'
import { useDashboardScope } from '../components/DashboardsLayout'
import { BigStat, DataTable, type DataColumn } from '../components/data'
import { Alert, Card, EmptyState, Spinner } from '../components/ui'
import { TimeChart, type ChartSeries } from '../lib/charts'
import { fetchDrives, type DistanceUnit, type DriveRow } from '../lib/dashboards'
import { useUnits } from '../lib/units'

interface DriveTableRow extends Record<string, unknown> {
  t: number
  drive_id: string
  from_t: number
  to_t: number
  distance: number | null
  duration_min: number | null
  energy_kwh: number | null
  avg_speed: number | null
  max_speed: number | null
  from_gf: string | null
  to_gf: string | null
  // Start location OR geofence, destination geofence OR address — combined
  // here so the table can sort on one value per column.
  from_loc: string | null
  to_loc: string | null
  start_battery: number | null
  end_battery: number | null
  power_max: number | null
  outside_temp: number | null
}

function toRow(r: DriveRow): DriveTableRow {
  return {
    t: r.t,
    drive_id: r.drive_id,
    from_t: r.from_t,
    to_t: r.to_t,
    distance: r.distance,
    duration_min: r.duration_min,
    energy_kwh: r.energy_kwh,
    avg_speed: r.avg_speed,
    max_speed: r.max_speed,
    from_gf: r.from_gf,
    to_gf: r.to_gf,
    from_loc: r.from_gf ?? r.start_address,
    to_loc: r.to_gf ?? r.end_address,
    start_battery: r.start_battery,
    end_battery: r.end_battery,
    power_max: r.power_max,
    outside_temp: r.outside_temp,
  }
}

// Humanized drive length: minutes below an hour, "1h 30m" above it.
// Mirrors the ChargeDetailView durationText.
function durationText(s: number | null): string {
  if (s == null) return '—'
  const mins = Math.round(s / 60)
  if (mins < 60) return `${mins} min`
  const h = Math.floor(mins / 60)
  const m = mins % 60
  return m === 0 ? `${h}h` : `${h}h ${m}m`
}

export function Drives() {
  const units = useUnits()
  const scope = useDashboardScope()
  const range = () => scope.range()
  // Distance unit always follows the global setting (Settings page);
  // there is no page-local override.
  const unit = (): DistanceUnit => (units.unitLength() === 'mi' ? 'mi' : 'km')
  // Speeds read "mph" in imperial (not "mi/h") and "km/h" in metric.
  const speedUnit = () => (unit() === 'mi' ? 'mph' : 'km/h')
  const vin = () => scope.vin()
  const navigate = useNavigate()

  const [drives] = createResource(
    () => {
      const v = vin()
      if (v == null) return false
      return { vin: v, range: range(), unit: unit() }
    },
    (s) => fetchDrives({ vin: s.vin, from: s.range.from, to: s.range.to, unit: s.unit }),
  )

  const rows = createMemo(() =>
    (drives()?.drives ?? []).map(toRow).sort((a, b) => b.from_t - a.from_t),
  )
  const isEmpty = () =>
    (drives()?.count ?? 0) === 0 &&
    (drives()?.drives.length ?? 0) === 0 &&
    (drives()?.per_day.length ?? 0) === 0

  const round1 = (v: number) => Math.round(v * 10) / 10
  const distanceText = () => `${round1(drives()?.distance ?? 0)} ${unit()}`
  // One decimal per the Grafana panel (decimals: 1).
  const energyText = () => {
    const v = drives()?.energy_kwh
    return v == null ? '—' : `${v.toFixed(1)} kWh`
  }

  const columns = createMemo((): Array<DataColumn<DriveTableRow>> => [
    { key: 'from_t', label: 'Date', render: (row) => new Date(row.from_t).toLocaleString() },
    { key: 'from_loc', label: 'Start', class: 'w-full break-words', render: (row) => row.from_loc ?? '—' },
    { key: 'to_loc', label: 'Destination', class: 'w-full break-words', render: (row) => row.to_loc ?? '—' },
    {
      key: 'duration_min',
      label: 'Duration',
      render: (row) => (row.duration_min == null ? '—' : durationText(row.duration_min * 60)),
    },
    {
      key: 'distance',
      label: 'Distance',
      align: 'right',
      render: (row) => (row.distance == null ? '—' : `${round1(row.distance)} ${unit()}`),
    },
    {
      key: 'start_battery',
      label: '% Start',
      align: 'right',
      render: (row) => (row.start_battery == null ? '—' : `${Math.round(row.start_battery)} %`),
    },
    {
      key: 'end_battery',
      label: '% End',
      align: 'right',
      render: (row) => (row.end_battery == null ? '—' : `${Math.round(row.end_battery)} %`),
    },
    {
      key: 'outside_temp',
      label: 'Temp',
      align: 'right',
      render: (row) => units.formatTemp(row.outside_temp),
    },
    {
      key: 'avg_speed',
      label: 'Ø Speed',
      align: 'right',
      render: (row) =>
        row.avg_speed == null ? '—' : `${round1(row.avg_speed)} ${speedUnit()}`,
    },
    {
      key: 'max_speed',
      label: 'Max Speed',
      align: 'right',
      render: (row) =>
        row.max_speed == null ? '—' : `${round1(row.max_speed)} ${speedUnit()}`,
    },
    {
      key: 'power_max',
      label: 'Max Power',
      align: 'right',
      render: (row) => (row.power_max == null ? '—' : `${round1(row.power_max)} kW`),
    },
  ])

  // Grafana renders Distance per Day as bars; TimeChart is line-only, so
  // this diverges visually (zeros still show correctly as zero points).
  const distanceSeries = createMemo((): Array<ChartSeries> => [
    {
      label: 'Distance',
      data: (drives()?.per_day ?? []).map((p): [number, number | null] => [p.t, p.value]),
      color: '#3b82f6',
    },
  ])

  return (
    <div>
      <Show when={vin()}>
        <Show when={drives.loading && !drives()}>
          <Spinner />
        </Show>
        <Show when={drives.error}>
          <div class="mb-4">
            <Alert tone="error">
              {drives.error instanceof Error ? drives.error.message : 'Failed to load drives data.'}
            </Alert>
          </div>
        </Show>
        <Show when={!drives.loading && !drives.error && drives() && isEmpty()}>
          <EmptyState
            title="No drives for this period"
            hint="Try a wider time range, or check back once the car completes a drive."
          />
        </Show>
        <Show when={!drives.error && drives() && !isEmpty()}>
          <div class="mb-4 grid gap-4 sm:grid-cols-2">
            <BigStat label="# of Drives" value={String(drives()?.count ?? 0)} />
            <BigStat label="Distance" value={distanceText()} />
            <BigStat label="Duration" value={durationText(drives()?.duration_s ?? null)} />
            <BigStat label="Energy Used" value={energyText()} />
          </div>
          <Card class="mb-4 p-5">
            <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">Drives</h2>
            <DataTable
              columns={columns()}
              rows={rows()}
              paginate={false}
              onRowClick={(row) => {
                if (!row.drive_id) return
                // The detail page (Stage 13) needs the drive's own window —
                // Grafana cannot auto-fit the axis to a variable — so the
                // row's from/to travels with the navigation.
                const from = new Date(row.from_t).toISOString()
                const to = new Date(row.to_t).toISOString()
                navigate(
                  `/dashboards/drives/${encodeURIComponent(row.drive_id)}?from=${encodeURIComponent(from)}&to=${encodeURIComponent(to)}`,
                )
              }}
            />
          </Card>
          <Card class="mb-4 p-5">
            <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">
              Distance per Day
            </h2>
            <TimeChart series={distanceSeries()} height={260} yUnit={unit()} />
          </Card>
        </Show>
      </Show>
    </div>
  )
}
