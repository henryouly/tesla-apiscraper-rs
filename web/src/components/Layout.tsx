import { A, useLocation } from '@solidjs/router'
import { Show, type ParentProps } from 'solid-js'
import { useAuth } from '../lib/auth'
import { useTheme } from '../lib/theme'

function ThemeToggle() {
  const { theme, setTheme } = useTheme()
  const next = () => setTheme(theme() === 'dark' ? 'light' : 'dark')
  return (
    <button
      onClick={next}
      title="Toggle dark mode"
      class="rounded border border-gray-300 px-2 py-1 text-sm dark:border-gray-700"
    >
      {theme() === 'dark' ? '☾' : '☀'}
    </button>
  )
}

export function Layout(props: ParentProps) {
  const auth = useAuth()
  const loc = useLocation()
  const authed = () => auth.authenticated() === true
  return (
    <div class="min-h-screen bg-gray-50 text-gray-900 dark:bg-gray-950 dark:text-gray-100">
      <nav class="flex items-center gap-4 border-b border-gray-200 px-4 py-2 dark:border-gray-800">
        <A href="/" class="text-lg font-bold">
          TeslaApiScraper
        </A>
        <Show when={authed()}>
          <A href="/settings" class="text-sm text-gray-600 dark:text-gray-300">
            Settings
          </A>
          <A href="/geofences" class="text-sm text-gray-600 dark:text-gray-300">
            Geofences
          </A>
        </Show>
        <span class="flex-1" />
        <Show when={loc.pathname !== '/signin' && !authed()}>
          <A href="/signin" class="text-sm text-gray-600 dark:text-gray-300">
            Sign in
          </A>
        </Show>
        <ThemeToggle />
      </nav>
      <Show when={auth.flash()}>
        <div class="border-b border-yellow-300 bg-yellow-100 px-4 py-2 text-sm dark:border-yellow-800 dark:bg-yellow-900">
          {auth.flash()}
        </div>
      </Show>
      <main class="mx-auto max-w-5xl p-4">{props.children}</main>
    </div>
  )
}
