import { useNavigate } from '@solidjs/router'
import { Show, createEffect, createSignal } from 'solid-js'
import { ApiError, api } from '../lib/api'
import { useAuth } from '../lib/auth'
import { Alert, Button, Card, FormField } from '../components/ui'

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
    <div class="mx-auto max-w-md pt-10">
      <div class="mb-6 text-center">
        <span class="mx-auto mb-4 flex h-12 w-12 items-center justify-center rounded-2xl bg-[#e82127] shadow-[0_12px_30px_-8px_rgba(232,33,39,0.8)]">
          <svg viewBox="0 0 24 24" class="h-6 w-6 text-white" fill="currentColor" aria-hidden="true">
            <path d="M13 2 4 14h6l-1 8 9-12h-6l1-8z" />
          </svg>
        </span>
        <p class="text-[11px] font-semibold uppercase tracking-[0.2em] text-[#e82127]">Tesla Scraper</p>
        <h1 class="mt-1 text-2xl font-bold tracking-tight text-zinc-50">Sign in</h1>
        <p class="mx-auto mt-2 max-w-sm text-sm text-zinc-400">
          Only needed if the server has no stored tokens (fresh setup, or the stored refresh token was
          revoked). Paste a Tesla refresh token — it is validated, then stored encrypted on the server.
        </p>
      </div>
      <Card class="p-5">
        <form onSubmit={submit} class="flex flex-col gap-3">
          <FormField
            label="Refresh token"
            type="password"
            value={refresh()}
            onInput={setRefresh}
            placeholder="Paste refresh token"
          />
          <Show when={error()}>
            <Alert tone="error">{error()}</Alert>
          </Show>
          <Button type="submit" loading={busy()} disabled={!refresh().trim()}>
            Sign in
          </Button>
        </form>
      </Card>
    </div>
  )
}
