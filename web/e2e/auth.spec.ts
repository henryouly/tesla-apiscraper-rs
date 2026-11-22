import { expect, mockApi, test } from './fixtures'

test.describe('Authentication & Sign-In', () => {
  test.use({ allowConsoleErrors: true })

  test('redirects unauthenticated visitors to /signin', async ({ page }) => {
    await mockApi(page, { authenticated: false })

    await page.goto('/')
    await expect(page).toHaveURL(/\/signin$/)
    await expect(page.getByRole('heading', { name: 'Sign in' })).toBeVisible()
    await expect(page.getByRole('button', { name: 'Sign in' })).toBeDisabled()
  })

  test('shows error on invalid refresh token and succeeds with valid token', async ({ page }) => {
    await mockApi(page, { authenticated: false })

    await page.goto('/signin')
    const input = page.getByPlaceholder('Paste refresh token')
    const submit = page.getByRole('button', { name: 'Sign in' })

    await input.fill('bad-token')
    await submit.click()
    await expect(page.getByText('invalid refresh token')).toBeVisible()
    await expect(page).toHaveURL(/\/signin$/)

    await input.fill('valid-refresh-token')
    await submit.click()
    await expect(page).toHaveURL(/\/$/)
    await expect(page.getByRole('heading', { name: 'Model 3' })).toBeVisible()
  })

  test('redirects back to /signin when a protected API returns 401 mid-session', async ({ page }) => {
    const api = await mockApi(page, { authenticated: true })

    await page.goto('/')
    await expect(page.getByRole('heading', { name: 'Model 3' })).toBeVisible()

    // Revoke tokens server-side mid-session, then navigate to Settings.
    api.state.authenticated = false
    await page.getByRole('link', { name: 'Settings' }).first().click()
    await expect(page).toHaveURL(/\/signin$/)
  })
})
