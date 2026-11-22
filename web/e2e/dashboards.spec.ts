import { expect, mockApi, test } from './fixtures'

test.describe('Native Dashboards & TimeRangePicker', () => {
  test('redirects /dashboards to overview, renders uPlot charts, and validates TimeRangePicker', async ({ page }) => {
    await mockApi(page)

    await page.goto('/dashboards')
    await expect(page).toHaveURL(/\/dashboards\/overview$/)
    await expect(page.getByRole('heading', { name: 'Overview', exact: true })).toBeVisible()
    await expect(page.locator('.uplot').first()).toBeVisible()

    // Open TimeRangePicker and switch preset to Last 24 hours
    await page.getByRole('button', { name: /Last 7 days/ }).click()
    await page.getByRole('button', { name: 'Last 24 hours' }).click()
    await expect(page.getByRole('button', { name: /Last 24 hours/ })).toBeVisible()

    // Open again and verify custom range validation (From after To)
    await page.getByRole('button', { name: /Last 24 hours/ }).click()
    await page.getByLabel('From').fill('2024-11-15T12:00')
    await page.getByLabel('To').fill('2024-11-14T12:00')
    await page.getByRole('button', { name: 'Apply' }).click()
    await expect(page.getByText('Enter a valid range with From before To.')).toBeVisible()
  })

  test('navigates all dashboard views and drill-downs with zero browser errors', async ({ page }) => {
    await mockApi(page)

    const views: Array<{ path: string; heading: string; expectSelector?: string; expectText?: string }> = [
      { path: '/dashboards/states', heading: 'States', expectText: 'online' },
      { path: '/dashboards/updates', heading: 'Updates', expectText: '2024.38.6' },
      { path: '/dashboards/mileage', heading: 'Mileage', expectSelector: '.uplot' },
      { path: '/dashboards/projected-range', heading: 'Projected Range', expectSelector: '.uplot' },
      { path: '/dashboards/efficiency', heading: 'Efficiency', expectText: '155' },
      { path: '/dashboards/locations', heading: 'Locations', expectText: 'Office' },
      { path: '/dashboards/drive-stats', heading: 'Drive Stats', expectText: 'Office' },
      { path: '/dashboards/drives', heading: 'Drives', expectText: 'Office' },
      { path: '/dashboards/drives/drv-100', heading: 'Drive Details', expectSelector: '.leaflet-container' },
      { path: '/dashboards/charges', heading: 'Charges', expectText: 'Home' },
      { path: '/dashboards/charging-stats', heading: 'Charging Stats', expectText: 'Supercharger' },
      {
        path: '/dashboards/charging-stats?charge_id=chg-1',
        heading: 'Charging Stats',
        expectSelector: '.uplot',
        expectText: 'Home',
      },
      { path: '/dashboards/trip', heading: 'Trip', expectSelector: '.leaflet-container' },
    ]

    for (const v of views) {
      await page.goto(v.path)
      await expect(
        page.getByRole('heading', { name: v.heading, level: 1, exact: true }),
        `Heading on ${v.path}`,
      ).toBeVisible()
      if (v.expectSelector) {
        await expect(
          page.locator(v.expectSelector).first(),
          `${v.expectSelector} on ${v.path}`,
        ).toBeVisible()
      }
      if (v.expectText) {
        await expect(
          page.getByText(v.expectText).first(),
          `Text "${v.expectText}" on ${v.path}`,
        ).toBeVisible()
      }
    }
  })
})
