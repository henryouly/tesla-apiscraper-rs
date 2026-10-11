import { Show, createSignal } from 'solid-js'
import { BigStat, DataTable, type DataColumn } from '../components/data'
import { Alert, Card, PageHeader } from '../components/ui'
import { TimeChart, type ChartSeries } from '../lib/charts'

interface DemoRow extends Record<string, unknown> {
  id: string
  name: string
  distance_mi: number
  duration_min: number
}

function fixtureSeries(): Array<ChartSeries> {
  const now = Date.now()
  const level: Array<[number, number]> = []
  const usable: Array<[number, number]> = []
  for (let i = 0; i <= 48; i++) {
    const t = now - (48 - i) * 3_600_000
    level.push([t, 55 + 20 * Math.sin(i / 6)])
    usable.push([t, 50 + 18 * Math.sin(i / 6 + 0.4)])
  }
  return [
    { label: 'Battery level %', data: level, color: '#10b981' },
    { label: 'Usable %', data: usable, color: '#0ea5e9' },
  ]
}

function fixtureRows(): Array<DemoRow> {
  return Array.from({ length: 30 }, (_, i) => ({
    id: `demo-${i + 1}`,
    name: `Trip ${i + 1}`,
    distance_mi: Math.round((8 + ((i * 37) % 120)) * 10) / 10,
    duration_min: 12 + ((i * 13) % 90),
  }))
}

const COLUMNS: Array<DataColumn<DemoRow>> = [
  { key: 'name', label: 'Trip' },
  { key: 'distance_mi', label: 'Distance (mi)', align: 'right' },
  { key: 'duration_min', label: 'Duration (min)', align: 'right' },
]

export function DashboardsDemo() {
  const [series] = createSignal(fixtureSeries())
  const [rows] = createSignal(fixtureRows())
  const [clicked, setClicked] = createSignal<string | null>(null)

  const flash = (row: DemoRow) => {
    setClicked(`${row.name}: ${String(row.distance_mi)} mi in ${String(row.duration_min)} min`)
    window.setTimeout(() => setClicked(null), 2500)
  }

  return (
    <div>
      <PageHeader
        eyebrow="Dashboards"
        title="Foundations demo"
        hint="Static fixtures for the Stage 0 building blocks: one TimeChart, two BigStats, one DataTable. Toggle the theme to check both palettes."
      />
      <Show when={clicked()}>
        <div class="mb-4">
          <Alert tone="info">{clicked()}</Alert>
        </div>
      </Show>
      <Card class="mb-4 p-5">
        <h2 class="mb-3 text-sm font-semibold text-zinc-700 dark:text-zinc-200">Battery · last 7 days fixture</h2>
        <TimeChart series={series()} height={260} yUnit="%" />
      </Card>
      <div class="mb-4 grid gap-4 sm:grid-cols-2">
        <BigStat label="Avg battery" value="68%" sub="last 7 days" />
        <BigStat label="Ideal range" value="212 mi" sub="latest sample" />
      </div>
      <DataTable columns={COLUMNS} rows={rows()} pageSize={10} onRowClick={flash} emptyTitle="No trips" />
    </div>
  )
}
