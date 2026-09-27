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

export interface BillingConfig {
  type: 'per_kwh' | 'per_minute'
  cost_per_unit: number
  session_fee: number
}

export interface Geofence {
  name: string
  latitude: number
  longitude: number
  radius_meters: number
  billing?: BillingConfig | null
}

export interface ChargeSession {
  charge_id: string
  [field: string]: unknown
}

export interface VehicleDiscovery {
  vin: string
  display_name: string | null
  state: string
}

export interface GlobalSettings {
  unit_length: string
  unit_temperature: string
  unit_pressure: string
  preferred_range: string
  language: string
  theme: string
}

export interface CarSettings {
  suspend_after_idle_minutes: number
  suspend_minimum_minutes: number
  require_unlocked_for_wake: boolean
  free_supercharging: boolean
  use_streaming_api: boolean
  enabled: boolean
  lfp_battery: boolean
}

export interface Settings {
  global: GlobalSettings
  cars: Record<string, CarSettings>
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
  settings(): Promise<{ settings: Settings }> {
    return req('/api/settings')
  },
  saveGlobalSettings(g: GlobalSettings): Promise<GlobalSettings> {
    return req('/api/settings/global', { method: 'PUT', body: JSON.stringify(g) })
  },
  saveCarSettings(vin: string, c: CarSettings): Promise<CarSettings> {
    return req(`/api/settings/cars/${encodeURIComponent(vin)}`, {
      method: 'PUT',
      body: JSON.stringify(c),
    })
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
  geofences(): Promise<{ geofences: Geofence[] }> {
    return req('/api/geofences')
  },
  createGeofence(g: Geofence): Promise<Geofence> {
    return req('/api/geofences', { method: 'POST', body: JSON.stringify(g) })
  },
  updateGeofence(name: string, g: Geofence): Promise<Geofence> {
    return req(`/api/geofences/${encodeURIComponent(name)}`, {
      method: 'PUT',
      body: JSON.stringify(g),
    })
  },
  deleteGeofence(name: string): Promise<void> {
    return req(`/api/geofences/${encodeURIComponent(name)}`, { method: 'DELETE' })
  },
  charge(id: string): Promise<ChargeSession> {
    return req(`/api/charges/${encodeURIComponent(id)}`)
  },  setChargeCost(
    id: string,
    mode: 'per_kwh' | 'per_minute',
    cost_per_unit: number,
    session_fee: number,
  ): Promise<{ charge_id: string; cost: number }> {
    return req(`/api/charges/${encodeURIComponent(id)}/cost`, {
      method: 'PUT',
      body: JSON.stringify({ mode, cost_per_unit, session_fee }),
    })
  },
}

/** Client-side cost preview. Mirrors the server formula (see docs/api.md). */
export function previewCost(
  mode: 'per_kwh' | 'per_minute',
  energyWh: number,
  durationSec: number,
  rate: number,
  fee: number,
): number {
  const base = mode === 'per_kwh' ? (energyWh / 1000) * rate : (durationSec / 60) * rate
  return Math.round((base + fee) * 100) / 100
}
