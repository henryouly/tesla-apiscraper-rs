import { expect, test } from './fixtures'

test.describe('Full-Stack E2E (Real Rust Server + InfluxDB 1.8 + Chromium)', () => {
  // Only allow the expected pre-sign-in 401 console log from UnitsProvider's
  // initial /api/settings fetch before authentication; uncaught pageerrors and
  // all other console.errors fail the test.
  test.use({ allowedConsoleErrors: [/status of 401/] })

  test('signs in, persists settings & geofences, edits charge cost in InfluxDB, and renders dashboard', async ({
    page,
  }) => {
    // Block external OpenStreetMap tile requests so Leaflet stays offline-fast.
    const transparentPng = Buffer.from(
      'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=',
      'base64',
    )
    await page.route('https://*.tile.openstreetmap.org/**', (route) =>
      route.fulfill({ status: 200, contentType: 'image/png', body: transparentPng }),
    )
    await page.route('https://tile.openstreetmap.org/**', (route) =>
      route.fulfill({ status: 200, contentType: 'image/png', body: transparentPng }),
    )

    // 1. Fresh Rust server has no stored tokens -> redirects to /signin.
    await page.goto('/')
    await expect(page).toHaveURL(/\/signin$/)
    await expect(page.getByRole('heading', { name: 'Sign in' })).toBeVisible()

    // 2. Sign in through real Rust /api/auth/sign_in -> discovers vehicle & redirects to /.
    await page.getByPlaceholder('Paste refresh token').fill('fullstack-refresh-token')
    await page.getByRole('button', { name: 'Sign in' }).click()
    await expect(page).toHaveURL(/\/$/)
    await expect(page.getByRole('heading', { name: 'Fullstack Model 3' })).toBeVisible()
    // Seeded InfluxDB position (battery_level = 77) is surfaced on the live card.
    await expect(page.getByText('77%')).toBeVisible()

    // 3. Persist global setting via real Rust /api/settings/global -> reload confirms YAML persistence.
    await page.getByRole('link', { name: 'Settings' }).first().click()
    await expect(page.getByRole('heading', { name: 'Settings', exact: true })).toBeVisible()
    await page.getByLabel('Unit of length').selectOption('km')
    await page.getByRole('button', { name: 'Save settings' }).click()
    await expect(page.getByText('Saved.')).toBeVisible()
    await page.reload()
    await expect(page.getByLabel('Unit of length')).toHaveValue('km')

    // 4. Create a geofence via real Rust /api/geofences -> reload confirms disk persistence.
    await page.getByRole('link', { name: 'Geofences' }).first().click()
    await expect(page.getByRole('heading', { name: 'Geofences', exact: true })).toBeVisible()
    await page.getByLabel('Name').fill('E2E Home')
    await page.getByLabel('Radius (m)').fill('120')
    await page.getByRole('button', { name: 'Create' }).click()
    await expect(page.getByRole('heading', { name: 'E2E Home', exact: true })).toBeVisible()
    await page.reload()
    await expect(page.getByRole('heading', { name: 'E2E Home', exact: true })).toBeVisible()

    // 5. Edit charge cost in real InfluxDB 1.8 via /charge/fs-chg-1/cost -> reload confirms DB rewrite.
    await page.goto('/charge/fs-chg-1/cost')
    await expect(page.getByText('40.0 kWh')).toBeVisible()
    await page.getByLabel('Cost per kWh').fill('0.30')
    await page.getByLabel('Session fee').fill('1.50')
    await expect(page.getByText('Preview: 13.50')).toBeVisible()
    await page.getByRole('button', { name: 'Save cost' }).click()
    await expect(page.getByText('Saved cost 13.50')).toBeVisible()
    await page.reload()
    await expect(page.getByText('13.5', { exact: true })).toBeVisible()

    // 6. Real dashboard queries against real InfluxDB 1.8 render Overview and Updates.
    await page.goto('/dashboards/overview')
    await expect(page.getByRole('heading', { name: 'Overview', level: 1, exact: true })).toBeVisible()
    await expect(page.locator('.uplot').first()).toBeVisible()

    await page.goto('/dashboards/updates')
    await expect(page.getByRole('heading', { name: 'Updates', level: 1, exact: true })).toBeVisible()
    await expect(page.getByText('2024.38.6').first()).toBeVisible()
    await expect(page.getByText('30 min').first()).toBeVisible()
    await expect(page.getByText('completed')).toBeVisible()
  })
})
