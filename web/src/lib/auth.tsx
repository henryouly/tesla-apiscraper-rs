import { useNavigate } from '@solidjs/router'
import {
  Show,
  createContext,
  createEffect,
  createResource,
  createSignal,
  useContext,
  type ParentProps,
} from 'solid-js'
import { api } from './api'

interface AuthState {
  authenticated: () => boolean | undefined
  refresh: () => void
  flash: () => string | null
  setFlash: (m: string | null) => void
}

const AuthContext = createContext<AuthState>()

export function AuthProvider(props: ParentProps) {
  const [status, { refetch }] = createResource(() =>
    api.authStatus().then((r) => r.authenticated),
  )
  const [flash, setFlash] = createSignal<string | null>(null)
  return (
    <AuthContext.Provider
      value={{
        authenticated: () => status(),
        refresh: () => refetch(),
        flash,
        setFlash,
      }}
    >
      {props.children}
    </AuthContext.Provider>
  )
}

export function useAuth() {
  const ctx = useContext(AuthContext)
  if (!ctx) throw new Error('useAuth outside AuthProvider')
  return ctx
}

/** Render children only when authenticated; otherwise go to /signin. */
export function RequireAuth(props: ParentProps) {
  const auth = useAuth()
  const nav = useNavigate()
  createEffect(() => {
    if (auth.authenticated() === false) nav('/signin', { replace: true })
  })
  return <Show when={auth.authenticated()}>{props.children}</Show>
}
