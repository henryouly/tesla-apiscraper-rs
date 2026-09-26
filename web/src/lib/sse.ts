import { onCleanup } from 'solid-js'
import type { UiEvent } from './api'

export type SseStatus = 'connecting' | 'live' | 'reconnecting'

/**
 * Subscribe to GET /api/events with auto-reconnect.
 *
 * EventSource reconnects by itself; this wrapper only tracks status for the
 * UI and re-attaches listeners on each underlying reconnect is unnecessary —
 * a single EventSource instance survives reconnects. On unmount the stream
 * is closed. Call `onEvent` for `summary`/`state`; `resync` asks the caller
 * to refetch `/api/vehicles/summaries`.
 */
export function useSse(
  onEvent: (ev: UiEvent) => void,
  onResync: () => void,
  onStatus: (s: SseStatus) => void,
) {
  const es = new EventSource('/api/events')
  onStatus('connecting')

  const handle = (e: MessageEvent) => {
    try {
      onEvent(JSON.parse(e.data as string) as UiEvent)
    } catch {
      // ignore malformed frames; next event resyncs via summary fetch
    }
  }
  const resync = () => onResync()

  es.addEventListener('summary', handle as EventListener)
  es.addEventListener('state', handle as EventListener)
  es.addEventListener('resync', resync)
  es.onopen = () => onStatus('live')
  es.onerror = () => {
    // EventSource is retrying underneath; surface it until onopen fires.
    if (es.readyState !== EventSource.OPEN) onStatus('reconnecting')
  }

  onCleanup(() => es.close())
  return es
}
