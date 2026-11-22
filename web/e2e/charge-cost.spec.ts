import { DEFAULT_VIN, expect, mockApi, test } from './fixtures'

test.describe('Charge Cost Editing', () => {
  test('previews and saves per_kwh and per_minute costs', async ({ page }) => {
    await mockApi(page)

    await page.goto('/charge/chg-1/cost')
    await expect(page.getByRole('heading', { name: 'Charge cost' })).toBeVisible()
    await expect(page.getByText('40.0 kWh')).toBeVisible()
    await expect(page.getByText('60 min')).toBeVisible()

    // per_kwh: 40 kWh * 0.30 + 1.50 = 13.50
    await page.getByLabel('Cost per kWh').fill('0.30')
    await page.getByLabel('Session fee').fill('1.50')
    await expect(page.getByText('Preview: 13.50')).toBeVisible()

    await page.getByRole('button', { name: 'Save cost' }).click()
    await expect(page.getByText('Saved cost 13.50')).toBeVisible()

    // Switch to per_minute: 60 min * 0.25 + 0 = 15.00
    await page.getByLabel('Billing mode').selectOption('per_minute')
    await page.getByLabel('Cost per minute').fill('0.25')
    await page.getByLabel('Session fee').fill('0')
    await expect(page.getByText('Preview: 15.00')).toBeVisible()

    await page.getByRole('button', { name: 'Save cost' }).click()
    await expect(page.getByText('Saved cost 15.00')).toBeVisible()
  })

  test.describe('error states', () => {
    test.use({ allowConsoleErrors: true })

    test('shows 404 alert for unknown session and 409 alert for open session', async ({ page }) => {
      await mockApi(page, {
        charges: {
          'chg-open': {
            charge_id: 'chg-open',
            vin: DEFAULT_VIN,
            energy_added_wh: 10_000,
            duration_seconds: null,
          },
        },
      })

      await page.goto('/charge/no-such-charge/cost')
      await expect(page.getByText('Session not found.')).toBeVisible()

      await page.goto('/charge/chg-open/cost')
      await page.getByRole('button', { name: 'Save cost' }).click()
      await expect(page.getByText(/session still open/)).toBeVisible()
    })
  })
})
