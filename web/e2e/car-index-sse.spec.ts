import { DEFAULT_VIN, expect, makeSummary, mockApi, test } from './fixtures'

test.describe('CarIndex & Live SSE', () => {
  test('renders vehicle card telemetry, Leaflet map, and toggles suspend/resume', async ({ page }) => {
    const api = await mockApi(page, {
      summaries: [
        makeSummary({
          display_name: 'Model 3',
          battery_level: 78,
          odometer: 12345,
          outside_temp: 18.5,
          inside_temp: 22.0,
          heading: 90,
          geofence_name: 'Home',
          sentry_mode: false,
          locked: true,
        }),
      ],
    })

    await page.goto('/')
    await expect(page.getByRole('heading', { name: 'Model 3' })).toBeVisible()
    await expect(page.getByText('78%')).toBeVisible()
    await expect(page.getByText('12345 mi')).toBeVisible()
    await expect(page.getByText('18.5 °C')).toBeVisible()
    await expect(page.getByText('Sentry off')).toBeVisible()
    await expect(page.getByText('90° E', { exact: true })).toBeVisible()
    await expect(page.getByText('Home')).toBeVisible()
    await expect(page.locator('.leaflet-container')).toBeVisible()

    // Suspend logging
    await page.getByRole('button', { name: 'Suspend logging' }).click()
    await expect(page.getByRole('button', { name: 'Resume logging' })).toBeVisible()
    await expect(page.getByText('Suspended').first()).toBeVisible()
    expect(
      api.state.requests.some(
        (r) => r.method === 'POST' && r.path === `/api/vehicles/${DEFAULT_VIN}/suspend`,
      ),
    ).toBe(true)

    // Resume logging
    await page.getByRole('button', { name: 'Resume logging' }).click()
    await expect(page.getByRole('button', { name: 'Suspend logging' })).toBeVisible()
    expect(
      api.state.requests.some(
        (r) => r.method === 'POST' && r.path === `/api/vehicles/${DEFAULT_VIN}/resume`,
      ),
    ).toBe(true)
  })

  test('applies live SSE summary frame to update battery and charging banner', async ({ page }) => {
    const updated = makeSummary({
      battery_level: 84,
      charging_state: 'Charging',
      charger_power: 11,
      charge_energy_added: 12.5,
      time_to_full_charge: 1.5,
    })
    const ssePayload = [
      'event: summary',
      `data: ${JSON.stringify({ type: 'summary', vin: DEFAULT_VIN, summary: updated, at: 1_700_000_120 })}`,
      '',
      '',
    ].join('\n')

    await mockApi(page, {
      summaries: [makeSummary({ battery_level: 78, charging_state: 'Disconnected' })],
      sseBody: ssePayload,
    })

    await page.goto('/')
    // Real EventSource receives the summary frame and updates the card live.
    await expect(page.getByText('84%')).toBeVisible()
    await expect(page.getByText('11 kW · +12.5 kWh · full in 1h 30m')).toBeVisible()
  })

  test('refetches summaries when SSE emits a resync frame', async ({ page }) => {
    let summaryFetches = 0
    await mockApi(page, {
      sseBody: 'event: resync\ndata: {"reason":"lagged"}\n\n',
      onRequest: (url, method) => {
        if (url.pathname === '/api/vehicles/summaries' && method === 'GET') {
          summaryFetches += 1
          return {
            json: {
              summaries: [
                makeSummary({
                  battery_level: summaryFetches === 1 ? 60 : 92,
                  last_updated_at: 1_700_000_000 + summaryFetches * 60,
                }),
              ],
            },
          }
        }
        return null
      },
    })

    await page.goto('/')
    await expect(page.getByText('92%')).toBeVisible()
    expect(summaryFetches).toBeGreaterThanOrEqual(2)
  })
})
