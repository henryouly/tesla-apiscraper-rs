import { expect, mockApi, test } from './fixtures'

test.describe('Geofences CRUD', () => {
  test('lists, creates with billing, edits, validates, and deletes geofences', async ({ page }) => {
    await mockApi(page)

    await page.goto('/geofences')
    await expect(page.getByRole('heading', { name: 'Geofences', exact: true })).toBeVisible()
    await expect(page.locator('.leaflet-container')).toBeVisible()
    await expect(page.getByRole('heading', { name: 'Home', exact: true })).toBeVisible()

    // Validation error on non-numeric latitude
    await page.getByLabel('Name').fill('Office')
    await page.getByLabel('Latitude').fill('not-a-number')
    await page.getByRole('button', { name: 'Create' }).click()
    await expect(page.getByText('Latitude, longitude, and radius must be numbers')).toBeVisible()

    // Create with per_kwh billing
    await page.getByLabel('Latitude').fill('37.8044')
    await page.getByLabel('Longitude').fill('-122.2712')
    await page.getByLabel('Radius (m)').fill('150')
    await page.getByLabel('Billing').selectOption('per_kwh')
    await page.getByLabel('Cost per kWh').fill('0.35')
    await page.getByLabel('Session fee').fill('1')
    await page.getByRole('button', { name: 'Create' }).click()

    await expect(page.getByRole('heading', { name: 'Office', exact: true })).toBeVisible()
    await expect(page.getByText('0.35/kWh + 1 fee')).toBeVisible()
    await expect(page.getByText('37.8044, -122.2712 · 150 m')).toBeVisible()

    // Edit Office radius
    const officeCard = page.locator('div', {
      has: page.getByRole('heading', { name: 'Office', exact: true }),
    }).filter({ has: page.getByRole('button', { name: 'Edit' }) }).last()
    await officeCard.getByRole('button', { name: 'Edit' }).click()
    await expect(page.getByRole('heading', { name: 'Edit Office' })).toBeVisible()
    await page.getByLabel('Radius (m)').fill('250')
    await page.getByRole('button', { name: 'Update' }).click()
    await expect(page.getByText('37.8044, -122.2712 · 250 m')).toBeVisible()

    // Two-step delete confirmation
    await officeCard.getByRole('button', { name: 'Delete' }).click()
    await officeCard.getByRole('button', { name: 'Confirm delete?' }).click()
    await expect(page.getByRole('heading', { name: 'Office', exact: true })).toHaveCount(0)
  })
})
