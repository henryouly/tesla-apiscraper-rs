// Typed client for the Rust backend (Phase 6 contracts).

export interface VehicleSummary {
  vin: string
  display_name: string | null
  state: string
  battery_level: number | null
  battery_range: number | null
  latitude: number | null
  longitude: number | null
  speed: number | null
  odometer: number | null
  last_updated_at: number
}

export interface UiSummaryEvent {
  type: 'summary'
  vin: string
  summary: VehicleSummary
  at: number
}

export interface UiStateEvent {
  type: 'state'
  vin: string
  state: string
  at: number
}

export type UiEvent = UiSummaryEvent | UiStateEvent

export interface VehicleDiscovery {
  vin: string
  display_name: string | null
  state: string
}

async function req<T>(path: string, init?: RequestInit): Promise<T> {
  const resp = await fetch(path, {
    headers: { 'content-type': 'application/json' },
    ...init,
  })
  if (resp.status === 401 && !location.pathname.startsWith('/signin')) {
    // Server-side token guard fired (e.g. tokens revoked mid-session).
    location.assign('/signin')
  }
  if (!resp.ok) {
    const body = await resp.text().catch(() => '')
    throw new ApiError(resp.status, body || resp.statusText)
  }
  if (resp.status === 204) return undefined as T
  return (await resp.json()) as T
}

export class ApiError extends Error {
  readonly status: number
  readonly body: string
  constructor(status: number, body: string) {
    super(`API ${status}: ${body}`)
    this.status = status
    this.body = body
  }
}

export const api = {
  authStatus(): Promise<{ authenticated: boolean }> {
    return req('/api/auth/status')
  },
  signIn(refresh_token: string) {
    return req<{ access_token: string; refresh_token: string; expires_in: number }>(
      '/api/auth/sign_in',
      { method: 'POST', body: JSON.stringify({ refresh_token }) },
    )
  },
  summaries(): Promise<{ summaries: VehicleSummary[] }> {
    return req('/api/vehicles/summaries')
  },
  vehicles(): Promise<{ vehicles: VehicleDiscovery[] }> {
    return req('/api/vehicles')
  },
  suspend(vin: string): Promise<void> {
    return req(`/api/vehicles/${encodeURIComponent(vin)}/suspend`, { method: 'POST' })
  },
  resume(vin: string): Promise<void> {
    return req(`/api/vehicles/${encodeURIComponent(vin)}/resume`, { method: 'POST' })
  },
}
