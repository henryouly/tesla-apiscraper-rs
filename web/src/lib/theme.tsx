import { createContext, createEffect, createSignal, useContext, type ParentProps } from 'solid-js'

export type Theme = 'light' | 'dark' | 'system'

const ThemeContext = createContext<{
  theme: () => Theme
  setTheme: (t: Theme) => void
  dark: () => boolean
}>()

function resolveDark(t: Theme): boolean {
  if (t === 'dark') return true
  if (t === 'light') return false
  return window.matchMedia('(prefers-color-scheme: dark)').matches
}

export function ThemeProvider(props: ParentProps) {
  const saved = (localStorage.getItem('theme') as Theme) || 'system'
  const [theme, setTheme] = createSignal<Theme>(saved)
  const [dark, setDark] = createSignal(resolveDark(saved))

  createEffect(() => {
    const t = theme()
    localStorage.setItem('theme', t)
    const d = resolveDark(t)
    setDark(d)
    document.documentElement.classList.toggle('dark', d)
  })

  // Follow OS changes while in system mode.
  const mq = window.matchMedia('(prefers-color-scheme: dark)')
  mq.addEventListener('change', () => {
    if (theme() === 'system') setDark(mq.matches)
    document.documentElement.classList.toggle('dark', resolveDark(theme()))
  })

  return (
    <ThemeContext.Provider value={{ theme, setTheme, dark }}>
      {props.children}
    </ThemeContext.Provider>
  )
}

export function useTheme() {
  const ctx = useContext(ThemeContext)
  if (!ctx) throw new Error('useTheme outside ThemeProvider')
  return ctx
}
