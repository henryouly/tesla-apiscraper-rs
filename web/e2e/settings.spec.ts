import { DEFAULT_VIN, expect, makeSettings, makeSummary, mockApi, test } from './fixtures'

test.describe('Settings & Unit Conversion', () => {
  test('updates global units and theme and applies them immediately to car cards', async ({ page }) => {
    await mockApi(page, {
      settings: makeSettings({
        global: { unit_length: 'mi', unit_temperature: 'C', theme: 'dark' },
      }),
      summaries: [makeSummary({ odometer: 100, outside_temp: 20.0 })],
    })

    await page.goto('/settings')
    await expect(page.getByRole('heading', { name: 'Settings', exact: true })).toBeVisible()

    await page.getByLabel('Unit of length').selectOption('km')
    await page.getByLabel('Unit of temperature').selectOption('F')
    await page.getByLabel('Theme').selectOption('dark')
    await expect(page.locator('html')).toHaveClass(/dark/)
    await page.getByLabel('Theme').selectOption('light')
    await expect(page.locator('html')).not.toHaveClass(/dark/)

    await page.getByRole('button', { name: 'Save settings' }).click()
    await expect(page.getByText('Saved.')).toBeVisible()

    // Navigate back to Home: odometer (100 mi -> 161 km) and outside temp (20°C -> 68.0 °F).
    await page.getByRole('link', { name: 'Cars' }).first().click()
    await expect(page).toHaveURL(/\/$/)
    await expect(page.getByText('161 km')).toBeVisible()
    await expect(page.getByText('68.0 °F')).toBeVisible()
  })

  test('validates and saves per-car settings', async ({ page }) => {
    const api = await mockApi(page)

    await page.goto(`/settings/car/${encodeURIComponent(DEFAULT_VIN)}`)
    await expect(page.getByRole('heading', { name: 'Model 3' })).toBeVisible()

    const idleInput = page.getByLabel('Suspend after idle (min)')
    await idleInput.fill('-5')
    await page.getByRole('button', { name: 'Save car settings' }).click()
    await expect(page.getByText('Suspend timers must be whole non-negative numbers')).toBeVisible()

    await idleInput.fill('30')
    await page.getByRole('switch', { name: /LFP battery/ }).click()
    await page.getByRole('button', { name: 'Save car settings' }).click()
    await expect(page.getByText('Saved.')).toBeVisible()

    expect(api.state.settings.cars[DEFAULT_VIN].suspend_after_idle_minutes).toBe(30)
    expect(api.state.settings.cars[DEFAULT_VIN].lfp_battery).toBe(true)
  })
})
