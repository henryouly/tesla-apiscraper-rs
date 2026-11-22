import { test as base, expect, type Page } from '@playwright/test'
import type {
  CarSettings,
  Geofence,
  GlobalSettings,
  Settings,
  VehicleDiscovery,
  VehicleSummary,
} from '../src/lib/api'

export { expect }

// 1x1 transparent PNG so Leaflet map tiles resolve offline with zero errors.
const TRANSPARENT_PNG = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=',
  'base64',
)

export const DEFAULT_VIN = '5YJ3E1EA1NF000001'

export function makeSummary(overrides: Partial<VehicleSummary> = {}): VehicleSummary {
  return {
    vin: DEFAULT_VIN,
    display_name: 'Model 3',
    state: 'Online',
    battery_level: 78,
    battery_range: 240,
    ideal_battery_range: 255,
    est_battery_range: 230,
    latitude: 37.7749,
    longitude: -122.4194,
    speed: 0,
    heading: 90,
    odometer: 12345,
    charging_state: 'Disconnected',
    shift_state: 'P',
    charge_energy_added: null,
    charge_limit_soc: 80,
    charger_power: null,
    time_to_full_charge: null,
    inside_temp: 22.0,
    outside_temp: 18.5,
    locked: true,
    sentry_mode: false,
    car_version: '2024.38.6',
    geofence_name: 'Home',
    last_updated_at: 1_700_000_000,
    ...overrides,
  }
}

export function makeSettings(overrides?: {
  global?: Partial<GlobalSettings>
  cars?: Record<string, CarSettings>
}): Settings {
  return {
    global: {
      unit_length: 'mi',
      unit_temperature: 'C',
      unit_pressure: 'bar',
      preferred_range: 'rated',
      language: 'en',
      theme: 'dark',
      ...overrides?.global,
    },
    cars: {
      [DEFAULT_VIN]: {
        suspend_after_idle_minutes: 21,
        suspend_minimum_minutes: 15,
        require_unlocked_for_wake: false,
        free_supercharging: false,
        use_streaming_api: true,
        enabled: true,
        lfp_battery: false,
      },
      ...overrides?.cars,
    },
  }
}

export interface MockApiOptions {
  authenticated?: boolean
  vehicles?: VehicleDiscovery[]
  summaries?: VehicleSummary[]
  settings?: Settings
  geofences?: Geofence[]
  charges?: Record<string, Record<string, unknown>>
  /** Pre-encoded SSE payload body served on GET /api/events. */
  sseBody?: string
  /** Optional custom handler for any /api/** request; return true if handled. */
  onRequest?: (url: URL, method: string, body: string | null) => { status?: number; json?: unknown; text?: string } | null
}

export interface MockApiController {
  state: {
    authenticated: boolean
    vehicles: VehicleDiscovery[]
    summaries: VehicleSummary[]
    settings: Settings
    geofences: Geofence[]
    charges: Record<string, Record<string, unknown>>
    sseBody: string
    requests: Array<{ method: string; path: string; body: string | null }>
  }
}

export async function mockApi(page: Page, options: MockApiOptions = {}): Promise<MockApiController> {
  const state: MockApiController['state'] = {
    authenticated: options.authenticated ?? true,
    vehicles: options.vehicles ?? [
      { vin: DEFAULT_VIN, display_name: 'Model 3', state: 'online' },
    ],
    summaries: options.summaries ?? [makeSummary()],
    settings: options.settings ?? makeSettings(),
    geofences: options.geofences ?? [
      {
        name: 'Home',
        latitude: 37.7749,
        longitude: -122.4194,
        radius_meters: 100,
        billing: { type: 'per_kwh', cost_per_unit: 0.25, session_fee: 0 },
      },
    ],
    charges: options.charges ?? {
      'chg-1': {
        charge_id: 'chg-1',
        vin: DEFAULT_VIN,
        energy_added_wh: 40_000,
        duration_seconds: 3600,
        cost: 10.0,
        geofence_name: 'Home',
      },
    },
    sseBody: options.sseBody ?? ': keep-alive\n\n',
    requests: [],
  }

  // Stub OpenStreetMap tiles so Leaflet never hits external networks.
  await page.route('https://*.tile.openstreetmap.org/**', (route) =>
    route.fulfill({ status: 200, contentType: 'image/png', body: TRANSPARENT_PNG }),
  )
  await page.route('https://tile.openstreetmap.org/**', (route) =>
    route.fulfill({ status: 200, contentType: 'image/png', body: TRANSPARENT_PNG }),
  )

  await page.route('**/api/**', async (route) => {
    const req = route.request()
    const url = new URL(req.url())
    const path = url.pathname
    const method = req.method()
    const body = req.postData()
    state.requests.push({ method, path, body })

    if (options.onRequest) {
      const custom = options.onRequest(url, method, body)
      if (custom) {
        if (custom.json !== undefined) {
          await route.fulfill({
            status: custom.status ?? 200,
            contentType: 'application/json',
            body: JSON.stringify(custom.json),
          })
        } else {
          await route.fulfill({
            status: custom.status ?? 200,
            contentType: 'text/plain',
            body: custom.text ?? '',
          })
        }
        return
      }
    }

    // Public auth endpoints
    if (path === '/api/auth/status' && method === 'GET') {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ authenticated: state.authenticated }),
      })
      return
    }
    if (path === '/api/auth/sign_in' && method === 'POST') {
      const parsed = body ? (JSON.parse(body) as { refresh_token?: string }) : {}
      if (parsed.refresh_token === 'valid-refresh-token') {
        state.authenticated = true
        await route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({
            access_token: 'at-123',
            refresh_token: 'rt-123',
            expires_in: 28800,
          }),
        })
      } else {
        await route.fulfill({
          status: 401,
          contentType: 'application/json',
          body: JSON.stringify({ error: 'invalid refresh token' }),
        })
      }
      return
    }

    // Server-side auth guard for protected routes
    if (!state.authenticated) {
      await route.fulfill({
        status: 401,
        contentType: 'application/json',
        body: JSON.stringify({ error: 'unauthorized' }),
      })
      return
    }

    // SSE stream
    if (path === '/api/events' && method === 'GET') {
      await route.fulfill({
        status: 200,
        contentType: 'text/event-stream',
        headers: { 'cache-control': 'no-cache', connection: 'keep-alive' },
        body: state.sseBody,
      })
      return
    }

    // Vehicles & summaries
    if (path === '/api/vehicles' && method === 'GET') {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ vehicles: state.vehicles }),
      })
      return
    }
    if (path === '/api/vehicles/summaries' && method === 'GET') {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ summaries: state.summaries }),
      })
      return
    }
    const suspendMatch = path.match(/^\/api\/vehicles\/([^/]+)\/(suspend|resume)$/)
    if (suspendMatch && method === 'POST') {
      const vin = decodeURIComponent(suspendMatch[1])
      const action = suspendMatch[2]
      const target = state.summaries.find((s) => s.vin === vin)
      if (target) {
        target.state = action === 'suspend' ? 'Suspended' : 'Online'
      }
      await route.fulfill({ status: 204, body: '' })
      return
    }

    // Settings
    if (path === '/api/settings' && method === 'GET') {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ settings: state.settings }),
      })
      return
    }
    if (path === '/api/settings/global' && method === 'PUT') {
      const next = JSON.parse(body ?? '{}') as GlobalSettings
      state.settings.global = { ...next }
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(state.settings.global),
      })
      return
    }
    const carSettingsMatch = path.match(/^\/api\/settings\/cars\/([^/]+)$/)
    if (carSettingsMatch && method === 'PUT') {
      const vin = decodeURIComponent(carSettingsMatch[1])
      const next = JSON.parse(body ?? '{}') as CarSettings
      state.settings.cars[vin] = { ...next }
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(state.settings.cars[vin]),
      })
      return
    }

    // Geofences CRUD
    if (path === '/api/geofences' && method === 'GET') {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ geofences: state.geofences }),
      })
      return
    }
    if (path === '/api/geofences' && method === 'POST') {
      const next = JSON.parse(body ?? '{}') as Geofence
      state.geofences.push(next)
      await route.fulfill({
        status: 201,
        contentType: 'application/json',
        body: JSON.stringify(next),
      })
      return
    }
    const gfMatch = path.match(/^\/api\/geofences\/([^/]+)$/)
    if (gfMatch) {
      const name = decodeURIComponent(gfMatch[1])
      if (method === 'PUT') {
        const next = JSON.parse(body ?? '{}') as Geofence
        const idx = state.geofences.findIndex((g) => g.name === name)
        if (idx >= 0) state.geofences[idx] = next
        await route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify(next),
        })
        return
      }
      if (method === 'DELETE') {
        state.geofences = state.geofences.filter((g) => g.name !== name)
        await route.fulfill({ status: 204, body: '' })
        return
      }
    }

    // Charges API
    const chargeCostMatch = path.match(/^\/api\/charges\/([^/]+)\/cost$/)
    if (chargeCostMatch && method === 'PUT') {
      const id = decodeURIComponent(chargeCostMatch[1])
      const existing = state.charges[id]
      if (!existing) {
        await route.fulfill({
          status: 404,
          contentType: 'application/json',
          body: JSON.stringify({ error: 'charge not found' }),
        })
        return
      }
      if (existing.duration_seconds == null) {
        await route.fulfill({
          status: 409,
          contentType: 'application/json',
          body: JSON.stringify({ error: 'session still open — retry after it closes' }),
        })
        return
      }
      const reqBody = JSON.parse(body ?? '{}') as {
        mode: 'per_kwh' | 'per_minute'
        cost_per_unit: number
        session_fee: number
      }
      const energy = Number(existing.energy_added_wh ?? 0)
      const dur = Number(existing.duration_seconds ?? 0)
      const baseCost =
        reqBody.mode === 'per_kwh'
          ? (energy / 1000) * reqBody.cost_per_unit
          : (dur / 60) * reqBody.cost_per_unit
      const cost = Math.round((baseCost + reqBody.session_fee) * 100) / 100
      existing.cost = cost
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ charge_id: id, cost }),
      })
      return
    }
    const chargeGetMatch = path.match(/^\/api\/charges\/([^/]+)$/)
    if (chargeGetMatch && method === 'GET') {
      const id = decodeURIComponent(chargeGetMatch[1])
      const existing = state.charges[id]
      if (!existing) {
        await route.fulfill({
          status: 404,
          contentType: 'application/json',
          body: JSON.stringify({ error: 'charge not found' }),
        })
        return
      }
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(existing),
      })
      return
    }

    // Dashboard endpoints
    if (path.startsWith('/api/dashboards/')) {
      const vin = url.searchParams.get('vin') ?? DEFAULT_VIN
      const payload = dashboardFixture(path, vin, url)
      if (payload) {
        await route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify(payload),
        })
        return
      }
    }

    await route.fulfill({
      status: 404,
      contentType: 'application/json',
      body: JSON.stringify({ error: `unmocked ${method} ${path}` }),
    })
  })

  return { state }
}

function dashboardFixture(path: string, vin: string, url: URL): unknown {
  const t0 = 1_700_000_000_000
  const t1 = t0 + 300_000
  switch (path) {
    case '/api/dashboards/overview':
      return {
        vin,
        battery: [
          { t: t0, level: 80, usable: 78 },
          { t: t1, level: 76, usable: 74 },
        ],
        ideal_range: [
          { t: t0, value: 320 },
          { t: t1, value: 304 },
        ],
      }
    case '/api/dashboards/states':
      return {
        vin,
        current: 'online',
        timeline: [
          { t: t0, state: 'asleep' },
          { t: t1, state: 'online' },
        ],
        changes: 2,
      }
    case '/api/dashboards/updates':
      return {
        vin,
        count: 1,
        updates: [
          {
            t: t0,
            version_before: '2024.32.6',
            version_after: '2024.38.6',
            install_start: '2024-11-14T10:00:00Z',
            install_end: '2024-11-14T10:30:00Z',
            status: 'installed',
          },
        ],
      }
    case '/api/dashboards/mileage':
      return {
        vin,
        points: [
          { t: t0, value: 12000 },
          { t: t1, value: 12050 },
        ],
      }
    case '/api/dashboards/projected-range':
      return {
        vin,
        projected: [
          { t: t0, ideal: 400, rated: 380 },
          { t: t1, ideal: 398, rated: 378 },
        ],
        outside_temp: [
          { t: t0, value: 19.5 },
          { t: t1, value: 20.0 },
        ],
      }
    case '/api/dashboards/efficiency':
      return {
        vin,
        avg_consumption_wh_per_km: 155.4,
        logged_distance: 42.5,
        per_drive: [
          { t: t0, value: 150.0 },
          { t: t1, value: 160.8 },
        ],
      }
    case '/api/dashboards/locations':
      return {
        vin,
        drives_logged: 3,
        top: [
          { place: 'Office', visits: 2 },
          { place: 'Home', visits: 1 },
        ],
        last_visited: [
          { t: t1, address: '123 Market St', geofence: 'Office', distance: 12.4 },
        ],
      }
    case '/api/dashboards/drive-stats':
      return {
        vin,
        drives: 2,
        distance: 35.0,
        energy_kwh: 5.8,
        median_drive: 17.5,
        max_speed: 112.0,
        speeds: [45, 65, 85, 105],
        top_destinations: [{ place: 'Office', visits: 2 }],
      }
    case '/api/dashboards/drives':
      return {
        vin,
        count: 1,
        distance: 25.0,
        duration_s: 1800,
        energy_kwh: 4.2,
        drives: [
          {
            t: t0,
            drive_id: 'drv-100',
            from_t: t0,
            to_t: t0 + 1_800_000,
            distance: 25.0,
            duration_min: 30,
            energy_kwh: 4.2,
            avg_speed: 50.0,
            max_speed: 95.0,
            from_gf: 'Home',
            to_gf: 'Office',
            start_address: 'San Francisco',
            end_address: 'Oakland',
            start_battery: 80,
            end_battery: 73,
            power_max: 85,
            outside_temp: 19.0,
          },
        ],
        per_day: [{ t: t0, value: 25.0 }],
      }
    case '/api/dashboards/drive-details':
      return {
        drive_id: url.searchParams.get('drive_id') ?? 'drv-100',
        window: { from: '2024-11-14T10:00:00Z', to: '2024-11-14T10:30:00Z' },
        series: [
          { t: t0, speed: 45, power: 25, battery: 80, range: 320 },
          { t: t1, speed: 60, power: 35, battery: 78, range: 312 },
        ],
        route: [
          { t: t0, lat: 37.7749, lng: -122.4194 },
          { t: t1, lat: 37.8044, lng: -122.2712 },
        ],
        elevation: [
          { t: t0, value: 12 },
          { t: t1, value: 24 },
        ],
        temps: [
          { t: t0, outside: 18.5, inside: 21.0 },
          { t: t1, outside: 19.0, inside: 21.5 },
        ],
        stats: { duration_s: 1800, distance: 25.0, energy_kwh: 4.2, avg_speed: 50.0 },
        odometer: { from: 12000, to: 12025 },
      }
    case '/api/dashboards/charges':
      return {
        vin,
        count: 1,
        energy_kwh: 40.0,
        total_cost: 10.0,
        avg_cost_per_kwh: 0.25,
        sessions: [
          {
            t: t0,
            charge_id: 'chg-1',
            location: 'Home',
            energy_kwh: 40.0,
            duration_min: 60,
            cost: 10.0,
            start_pct: 30,
            end_pct: 80,
          },
        ],
        energy_per_day: [{ t: t0, value: 40.0 }],
        power_by_session: [{ charge_id: 'chg-1', points: [{ t: t0, value: 11.0 }] }],
      }
    case '/api/dashboards/charging-stats':
      return {
        vin,
        count: 2,
        energy_kwh: 75.0,
        total_cost: 22.5,
        avg_cost_per_kwh: 0.3,
        soc_delta: [{ t: t0, start: 25, end: 80 }],
        top_stations: [
          { station: 'Home', energy_kwh: 45.0, sessions: 1 },
          { station: 'Supercharger', energy_kwh: 30.0, sessions: 1 },
        ],
      }
    case '/api/dashboards/charge-details':
      return {
        charge_id: url.searchParams.get('charge_id') ?? 'chg-1',
        cost: 10.0,
        duration_s: 3600,
        geofence: 'Home',
        energy_kwh: 40.0,
        lat: 37.7749,
        lng: -122.4194,
        soc_added: 50,
        start_t: t0,
        end_t: t0 + 3_600_000,
        curve: [
          { t: t0, battery: 30, power_kw: 11.0, added_kwh: 0 },
          { t: t1, battery: 35, power_kw: 11.2, added_kwh: 5.5 },
        ],
      }
    case '/api/dashboards/trip':
      return {
        vin,
        distance: 80.0,
        driving_s: 3600,
        charging_s: 1200,
        avg_speed: 80.0,
        consumption_wh_per_km: 165.0,
        charging_cost: 12.5,
        drives: [
          {
            t: t0,
            drive_id: 'drv-100',
            from_t: t0,
            to_t: t0 + 3_600_000,
            distance: 80.0,
            duration_min: 60,
            energy_kwh: 13.2,
            avg_speed: 80.0,
            max_speed: 110.0,
            from_gf: 'Home',
            to_gf: 'Office',
            start_address: 'San Francisco',
            end_address: 'San Jose',
            start_battery: 85,
            end_battery: 55,
            power_max: 95,
            outside_temp: 20.0,
          },
        ],
        charges: [
          { t: t1, charge_id: 'chg-1', location: 'San Jose SC', energy_kwh: 30.0, cost: 12.5 },
        ],
        battery: [
          { t: t0, battery: 85, ideal_range: 340 },
          { t: t1, battery: 55, ideal_range: 220 },
        ],
        route: [
          { t: t0, lat: 37.7749, lng: -122.4194 },
          { t: t1, lat: 37.3382, lng: -121.8863 },
        ],
      }
    default:
      return null
  }
}

export const test = base.extend<{ allowConsoleErrors: boolean }>({
  allowConsoleErrors: [false, { option: true }],
  page: async ({ page, allowConsoleErrors }, use) => {
    const errors: string[] = []
    page.on('pageerror', (err) => errors.push(`pageerror: ${err.message}`))
    page.on('console', (msg) => {
      if (msg.type() === 'error') {
        errors.push(`console.error: ${msg.text()}`)
      }
    })
    await use(page)
    if (!allowConsoleErrors) {
      expect(errors, `Unexpected browser errors:\n${errors.join('\n')}`).toEqual([])
    }
  },
})
