import { For, Show, createMemo, createSignal, type JSX } from 'solid-js'
import { Button, Card, EmptyState } from './ui'

/* ---------- BigStat: Grafana-stat style, label left / big value right ---------- */

export function BigStat(props: { label: string; value: string; sub?: string }) {
  return (
    <div class="flex items-center justify-between gap-4 rounded-2xl border border-zinc-200 bg-white px-5 py-4 shadow-sm dark:border-white/[0.07] dark:bg-[#101013]/90">
      <div class="min-w-0">
        <p class="text-[11px] font-medium uppercase tracking-wider text-zinc-500 dark:text-zinc-400">{props.label}</p>
        <Show when={props.sub}>
          <p class="mt-0.5 truncate text-xs text-zinc-500 dark:text-zinc-400">{props.sub}</p>
        </Show>
      </div>
      <p class="shrink-0 text-3xl font-bold tabular-nums tracking-tight text-zinc-900 dark:text-zinc-50">
        {props.value}
      </p>
    </div>
  )
}

/* ---------- DataTable: sortable, paginated ---------- */

export interface DataColumn<T extends Record<string, unknown>> {
  key: string
  label: string
  align?: 'left' | 'center' | 'right'
  render?: (row: T) => JSX.Element
}

interface DataTableProps<T extends Record<string, unknown>> {
  columns: Array<DataColumn<T>>
  rows: Array<T>
  pageSize?: number
  onRowClick?: (row: T) => void
  emptyTitle?: string
  emptyHint?: string
}

function compareValues(a: unknown, b: unknown): number {
  if (a == null && b == null) return 0
  if (a == null) return 1
  if (b == null) return -1
  if (typeof a === 'number' && typeof b === 'number') return a - b
  return String(a).localeCompare(String(b), undefined, { numeric: true })
}

function cellText<T extends Record<string, unknown>>(row: T, key: string): string {
  const v: unknown = row[key]
  return v == null ? '—' : String(v)
}

function alignClass(align: DataColumn<Record<string, unknown>>['align']): string {
  switch (align) {
    case 'center':
      return 'text-center'
    case 'right':
      return 'text-right'
    default:
      return 'text-left'
  }
}

export function DataTable<T extends Record<string, unknown>>(props: DataTableProps<T>) {
  const [sortKey, setSortKey] = createSignal<string | null>(null)
  const [sortDir, setSortDir] = createSignal<1 | -1>(1)
  const [page, setPage] = createSignal(0)

  const pageSize = () => props.pageSize ?? 25

  const sorted = createMemo(() => {
    const key = sortKey()
    const rows = [...props.rows]
    if (key == null) return rows
    const dir = sortDir()
    rows.sort((a, b) => compareValues(a[key], b[key]) * dir)
    return rows
  })

  const totalPages = createMemo(() => Math.max(1, Math.ceil(sorted().length / pageSize())))

  const paged = createMemo(() => {
    const p = Math.min(page(), totalPages() - 1)
    return sorted().slice(p * pageSize(), (p + 1) * pageSize())
  })

  const pageLabel = () => (sorted().length === 0 ? 'Page 0 of 0' : `Page ${Math.min(page(), totalPages() - 1) + 1} of ${totalPages()}`)

  const toggleSort = (key: string) => {
    if (sortKey() === key) {
      setSortDir((d) => (d === 1 ? -1 : 1))
    } else {
      setSortKey(key)
      setSortDir(1)
    }
    setPage(0)
  }

  const sortMark = (key: string) => {
    if (sortKey() !== key) return ''
    return sortDir() === 1 ? ' ▲' : ' ▼'
  }

  return (
    <Show
      when={props.rows.length > 0}
      fallback={<EmptyState title={props.emptyTitle ?? 'No rows'} hint={props.emptyHint} />}
    >
      <Card class="overflow-hidden">
        <div class="overflow-x-auto">
          <table class="w-full text-sm">
            <thead>
              <tr class="border-b border-zinc-200 dark:border-white/[0.07]">
                <For each={props.columns}>
                  {(col) => (
                    <th class={`px-4 py-2.5 font-medium text-zinc-500 dark:text-zinc-400 ${alignClass(col.align)}`}>
                      <button
                        type="button"
                        onClick={() => toggleSort(col.key)}
                        class="rounded px-1 py-0.5 transition-colors hover:bg-zinc-900/[0.05] hover:text-zinc-900 dark:hover:bg-white/[0.06] dark:hover:text-zinc-100"
                        title={`Sort by ${col.label}`}
                      >
                        {col.label}
                        {sortMark(col.key)}
                      </button>
                    </th>
                  )}
                </For>
              </tr>
            </thead>
            <tbody>
              <For each={paged()}>
                {(row) => (
                  <tr
                    onClick={() => props.onRowClick?.(row)}
                    onKeyDown={(e) => {
                      if (props.onRowClick && (e.key === 'Enter' || e.key === ' ')) {
                        e.preventDefault()
                        props.onRowClick(row)
                      }
                    }}
                    tabIndex={props.onRowClick ? 0 : undefined}
                    class={`border-b border-zinc-100 outline-none last:border-0 dark:border-white/[0.04] ${
                      props.onRowClick
                        ? 'cursor-pointer transition-colors hover:bg-zinc-900/[0.03] focus-visible:bg-zinc-900/[0.03] focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-[#e82127]/40 dark:hover:bg-white/[0.04] dark:focus-visible:bg-white/[0.04]'
                        : ''
                    }`}
                  >
                    <For each={props.columns}>
                      {(col) => (
                        <td class={`px-4 py-2.5 text-zinc-800 tabular-nums dark:text-zinc-200 ${alignClass(col.align)}`}>
                          {col.render ? col.render(row) : cellText(row, col.key)}
                        </td>
                      )}
                    </For>
                  </tr>
                )}
              </For>
            </tbody>
          </table>
        </div>
        <Show when={totalPages() > 1}>
          <div class="flex items-center justify-between gap-2 border-t border-zinc-200 px-4 py-2.5 dark:border-white/[0.07]">
            <p class="text-xs text-zinc-500 dark:text-zinc-400">{pageLabel()}</p>
            <div class="flex items-center gap-2">
              <Button
                variant="secondary"
                size="sm"
                disabled={page() === 0}
                onClick={() => setPage((p) => Math.max(0, p - 1))}
              >
                Prev
              </Button>
              <Button
                variant="secondary"
                size="sm"
                disabled={page() >= totalPages() - 1}
                onClick={() => setPage((p) => Math.min(totalPages() - 1, p + 1))}
              >
                Next
              </Button>
            </div>
          </div>
        </Show>
      </Card>
    </Show>
  )
}
