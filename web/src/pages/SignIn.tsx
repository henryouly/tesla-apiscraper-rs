import { useNavigate } from '@solidjs/router'
import { Show, createEffect, createSignal } from 'solid-js'
import { ApiError, api } from '../lib/api'
import { useAuth } from '../lib/auth'
import { Button, Card, FormField } from '../components/ui'

export function SignIn() {
  const auth = useAuth()
  const nav = useNavigate()
  const [refresh, setRefresh] = createSignal('')
  const [error, setError] = createSignal<string | null>(null)
  const [busy, setBusy] = createSignal(false)

  createEffect(() => {
    if (auth.authenticated() === true) nav('/', { replace: true })
  })

  const submit = async (e: Event) => {
    e.preventDefault()
    setBusy(true)
    setError(null)
    try {
      await api.signIn(refresh().trim())
      auth.refresh()
      nav('/', { replace: true })
    } catch (err) {
      setError(err instanceof ApiError ? err.body : 'Sign-in failed')
    } finally {
      setBusy(false)
    }
  }

  return (
    <div class="mx-auto max-w-md">
      <Card>
        <h1 class="mb-4 text-xl font-bold">Sign in</h1>
        <p class="mb-4 text-sm text-gray-600 dark:text-gray-400">
          Only needed if the server has no stored tokens (fresh setup, or the stored refresh
          token was revoked). Paste a Tesla refresh token — it is validated, then stored
          encrypted on the server. A fresh access token is minted from it automatically.
        </p>
        <form onSubmit={submit} class="flex flex-col gap-3">
          <FormField
            label="Refresh token"
            type="password"
            value={refresh()}
            onInput={setRefresh}
            placeholder="refresh token"
          />
          <Show when={error()}>
            <p class="text-sm text-red-600">{error()}</p>
          </Show>
          <Button type="submit" disabled={busy() || !refresh().trim()}>
            {busy() ? 'Signing in…' : 'Sign in'}
          </Button>
        </form>
      </Card>
    </div>
  )
}
