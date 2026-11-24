export type UpdateBadgeTone = 'green' | 'amber' | 'red' | 'gray'

export interface UpdateStatusBadge {
  label: string
  tone: UpdateBadgeTone
}

/**
 * Compute non-negative install duration in seconds from RFC3339 start/end
 * timestamps. Returns null if either timestamp is missing, invalid, or
 * `end < start`.
 */
export function computeInstallDurationSec(
  startIso: string | null | undefined,
  endIso: string | null | undefined,
): number | null {
  if (!startIso || !endIso) return null
  const startMs = Date.parse(startIso)
  const endMs = Date.parse(endIso)
  if (!Number.isFinite(startMs) || !Number.isFinite(endMs) || endMs < startMs) {
    return null
  }
  return Math.round((endMs - startMs) / 1000)
}

/** Format an install duration in seconds as `45s`, `30 min`, `1h 15m`, or `—`. */
export function formatUpdateDuration(sec: number | null): string {
  if (sec == null || !Number.isFinite(sec) || sec < 0) return '—'
  if (sec < 60) return `${Math.round(sec)}s`
  const mins = Math.round(sec / 60)
  if (mins < 60) return `${mins} min`
  const h = Math.floor(mins / 60)
  const m = mins % 60
  return m === 0 ? `${h}h` : `${h}h ${m}m`
}

/** Format an RFC3339 timestamp in the user's locale, or `—` when absent/invalid. */
export function formatUpdateTimestamp(iso: string | null | undefined): string {
  if (!iso) return '—'
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return iso
  return d.toLocaleString(undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  })
}

/** Map raw update status + abandoned flag into a display label and Pill tone. */
export function updateStatusBadge(
  status: string | null | undefined,
  abandoned?: boolean | null,
): UpdateStatusBadge {
  if (abandoned === true) {
    return { label: 'abandoned', tone: 'red' }
  }
  const normalized = (status ?? '').trim().toLowerCase()
  switch (normalized) {
    case 'completed':
    case 'installed':
      return { label: normalized, tone: 'green' }
    case 'installing':
      return { label: normalized, tone: 'amber' }
    case 'cancelled':
    case 'canceled':
    case 'failed':
      return { label: normalized, tone: 'red' }
    default:
      return { label: normalized || 'unknown', tone: 'gray' }
  }
}
