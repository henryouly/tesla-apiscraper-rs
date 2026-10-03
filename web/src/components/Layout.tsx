import { A, useLocation } from '@solidjs/router'
import { Show, type ParentProps } from 'solid-js'
import { useAuth } from '../lib/auth'
import { useTheme } from '../lib/theme'
import { I, Icon } from './ui'

function Brand() {
  return (
    <A href="/" class="group flex items-center gap-2.5">
      <span class="flex h-8 w-8 items-center justify-center rounded-[10px] bg-[#e82127] shadow-[0_8px_20px_-6px_rgba(232,33,39,0.8)]">
        <svg viewBox="0 0 24 24" class="h-[18px] w-[18px] text-white" fill="currentColor" aria-hidden="true">
          <path d="M13 2 4 14h6l-1 8 9-12h-6l1-8z" />
        </svg>
      </span>
      <span class="leading-tight">
        <span class="block text-[15px] font-bold tracking-tight text-zinc-900 dark:text-zinc-50">Tesla Scraper</span>
        <span class="block text-[10px] font-medium uppercase tracking-[0.2em] text-zinc-500">Local telemetry</span>
      </span>
    </A>
  )
}

function NavLink(props: { href: string; children: string }) {
  const loc = useLocation()
  const active = () => loc.pathname === props.href || (props.href !== '/' && loc.pathname.startsWith(props.href))
  return (
    <A
      href={props.href}
      aria-current={active() ? 'page' : undefined}
      class={`rounded-lg px-3 py-1.5 text-sm font-medium transition-colors ${
        active()
          ? 'bg-zinc-900/[0.06] text-zinc-900 dark:bg-white/[0.08] dark:text-zinc-50'
          : 'text-zinc-500 hover:bg-zinc-900/[0.04] hover:text-zinc-900 dark:text-zinc-400 dark:hover:bg-white/[0.05] dark:hover:text-zinc-100'
      }`}
    >
      {props.children}
    </A>
  )
}

function ThemeToggle() {
  const { setTheme, dark } = useTheme()
  return (
    <button
      onClick={() => setTheme(dark() ? 'light' : 'dark')}
      title={dark() ? 'Switch to light mode' : 'Switch to dark mode'}
      class="flex h-9 w-9 items-center justify-center rounded-xl border border-zinc-300 bg-white text-zinc-500 transition-colors hover:border-zinc-400 hover:text-zinc-800 dark:border-white/10 dark:bg-white/[0.04] dark:text-zinc-400 dark:hover:border-white/20 dark:hover:text-zinc-100"
    >
      <Show when={dark()} fallback={<Icon d={I.moon} />}>
        <Icon d={I.sun} />
      </Show>
    </button>
  )
}

export function Layout(props: ParentProps) {
  const auth = useAuth()
  const loc = useLocation()
  const authed = () => auth.authenticated() === true
  return (
    <div class="app-backdrop min-h-screen bg-zinc-100 text-zinc-900 antialiased dark:bg-[#09090b] dark:text-zinc-100">
      <header class="sticky top-0 z-40 border-b border-zinc-200 bg-white/80 backdrop-blur-xl dark:border-white/[0.07] dark:bg-[#09090b]/80">
        <nav class="mx-auto flex h-16 max-w-6xl items-center gap-2 px-4">
          <Brand />
          <div class="ml-4 hidden items-center gap-1 sm:flex">
            <Show when={authed()}>
              <NavLink href="/">Cars</NavLink>
              <NavLink href="/geofences">Geofences</NavLink>
              <NavLink href="/settings">Settings</NavLink>
            </Show>
          </div>
          <span class="flex-1" />
          <Show when={loc.pathname !== '/signin' && !authed()}>
            <A href="/signin" class="rounded-lg px-3 py-1.5 text-sm font-medium text-zinc-600 hover:bg-zinc-900/[0.05] hover:text-zinc-900 dark:text-zinc-300 dark:hover:bg-white/[0.06] dark:hover:text-white">
              Sign in
            </A>
          </Show>
          <ThemeToggle />
        </nav>
        <Show when={authed()}>
          <div class="border-t border-zinc-200 dark:border-white/[0.05] sm:hidden">
            <div class="mx-auto flex max-w-6xl gap-1 px-4 py-2">
              <NavLink href="/">Cars</NavLink>
              <NavLink href="/geofences">Geofences</NavLink>
              <NavLink href="/settings">Settings</NavLink>
            </div>
          </div>
        </Show>
      </header>
      <Show when={auth.flash()}>
        <div class="border-b border-amber-600/20 bg-amber-500/10 px-4 py-2.5 text-center text-sm text-amber-800 dark:border-amber-500/20 dark:text-amber-200">
          {auth.flash()}
        </div>
      </Show>
      <main class="mx-auto max-w-6xl animate-[fade-up_0.45s_cubic-bezier(0.22,1,0.36,1)_both] px-4 pb-16 pt-6">
        {props.children}
      </main>
      <footer class="border-t border-zinc-200 py-6 dark:border-white/[0.06]">
        <p class="mx-auto max-w-6xl px-4 text-xs text-zinc-500 dark:text-zinc-400">
          Tesla Scraper · local-first Tesla telemetry — drives, charges, geofences.
        </p>
      </footer>
    </div>
  )
}
