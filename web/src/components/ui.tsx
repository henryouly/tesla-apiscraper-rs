import type { JSX, ParentProps } from 'solid-js'
import { For, Show } from 'solid-js'

/* ---------- Icons (inline SVG, no deps) ---------- */

export function Icon(props: { d: string; class?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" class={props.class ?? 'h-4 w-4'} aria-hidden="true">
      <path d={props.d} />
    </svg>
  )
}

export const I = {
  bolt: 'M13 2 4 14h6l-1 8 9-12h-6l1-8z',
  pin: 'M12 21s-7-5.5-7-11a7 7 0 0 1 14 0c0 5.5-7 11-7 11z M12 12.5a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5z',
  gear: 'M4 8h9 M17 8h3 M4 16h3 M11 16h9 M14 5v6 M8 13v6',
  car: 'M5 16 6.5 9.5A2 2 0 0 1 8.5 8h7a2 2 0 0 1 2 1.5L19 16 M5 16h14 M5 16v3 M19 16v3 M7.5 12.5h.01 M16.5 12.5h.01',
  plus: 'M12 5v14 M5 12h14',
  search: 'M11 5a6 6 0 1 0 4.2 10.3L20 20.2',
  sun: 'M12 17a5 5 0 1 0 0-10 5 5 0 0 0 0 10z M12 2v2 M12 20v2 M4 12H2 M22 12h-2 M5 5l1.5 1.5 M17.5 17.5 19 19 M19 5l-1.5 1.5 M6.5 17.5 5 19',
  moon: 'M20 14.5A8 8 0 0 1 9.5 4 8 8 0 1 0 20 14.5z',
  back: 'M15 5l-7 7 7 7',
  check: 'M4 12.5l5 5L20 6.5',
}

/* ---------- Buttons ---------- */

type BtnVariant = 'primary' | 'secondary' | 'ghost' | 'danger'
type BtnSize = 'sm' | 'md'

export function Button(props: {
  type?: 'button' | 'submit'
  disabled?: boolean
  loading?: boolean
  variant?: BtnVariant
  size?: BtnSize
  onClick?: () => void
  children: JSX.Element
}) {
  const variant = () => props.variant ?? 'primary'
  const size = () => props.size ?? 'md'
  const cls = () => {
    const base =
      'inline-flex items-center justify-center gap-1.5 rounded-lg font-medium transition-all focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-red-500 active:scale-[0.98] disabled:pointer-events-none disabled:opacity-50'
    const sz = size() === 'sm' ? 'px-2.5 py-1.5 text-xs' : 'px-3.5 py-2 text-sm'
    switch (variant()) {
      case 'primary':
        return `${base} ${sz} bg-[#e82127] text-white shadow-[0_8px_20px_-8px_rgba(232,33,39,0.7)] hover:bg-[#c81a20]`
      case 'secondary':
        return `${base} ${sz} border border-zinc-300 bg-white text-zinc-700 hover:bg-zinc-100 dark:border-white/10 dark:bg-white/[0.06] dark:text-zinc-100 dark:hover:bg-white/[0.1]`
      case 'ghost':
        return `${base} ${sz} text-zinc-500 hover:bg-zinc-900/[0.05] hover:text-zinc-900 dark:text-zinc-400 dark:hover:bg-white/[0.06] dark:hover:text-zinc-100`
      case 'danger':
        return `${base} ${sz} bg-red-500/10 text-red-600 ring-1 ring-inset ring-red-500/30 hover:bg-red-500/20 dark:text-red-400`
    }
  }
  return (
    <button
      type={props.type ?? 'button'}
      disabled={props.disabled || props.loading}
      onClick={() => props.onClick?.()}
      class={cls()}
    >
      <Show when={props.loading}>
        <span class="h-3.5 w-3.5 animate-spin rounded-full border-2 border-current border-t-transparent opacity-60" />
      </Show>
      {props.children}
    </button>
  )
}

/* ---------- Cards / layout ---------- */

export function Card(props: ParentProps<{ class?: string; hover?: boolean }>) {
  return (
    <div
      class={`rounded-2xl border border-zinc-200 bg-white shadow-sm backdrop-blur transition-colors dark:border-white/[0.07] dark:bg-[#101013]/90 dark:shadow-[0_20px_50px_-30px_rgba(0,0,0,0.8)] ${
        props.hover ? 'hover:border-zinc-300 dark:hover:border-white/[0.14]' : ''
      } ${props.class ?? ''}`}
    >
      {props.children}
    </div>
  )
}

export function PageHeader(props: {
  eyebrow?: string
  title: string
  hint?: string
  actions?: JSX.Element
}) {
  return (
    <div class="mb-5">
      <Show when={props.eyebrow}>
        <p class="mb-1 text-[11px] font-semibold uppercase tracking-[0.18em] text-[#e82127]">
          {props.eyebrow}
        </p>
      </Show>
      <div class="flex flex-wrap items-center gap-3">
        <h1 class="text-2xl font-bold tracking-tight text-zinc-900 dark:text-zinc-50">{props.title}</h1>
        <Show when={props.actions}>
          <div class="flex items-center gap-2">{props.actions}</div>
        </Show>
      </div>
      <Show when={props.hint}>
        <p class="mt-1 max-w-xl text-sm text-zinc-500 dark:text-zinc-400">{props.hint}</p>
      </Show>
    </div>
  )
}

export function Stat(props: { label: string; value: string; sub?: string }) {
  return (
    <div class="rounded-xl border border-zinc-200 bg-zinc-50 px-3 py-2.5 dark:border-white/[0.06] dark:bg-white/[0.03]">
      <p class="text-[11px] font-medium uppercase tracking-wider text-zinc-500">{props.label}</p>
      <p class="mt-0.5 truncate text-sm font-semibold text-zinc-900 dark:text-zinc-100">{props.value}</p>
      <Show when={props.sub}>
        <p class="truncate text-xs text-zinc-500">{props.sub}</p>
      </Show>
    </div>
  )
}

/* ---------- Status pill ---------- */

export function Pill(props: { tone?: 'green' | 'blue' | 'gray' | 'amber' | 'red'; pulse?: boolean; children: JSX.Element }) {
  const tone = () => props.tone ?? 'gray'
  const dot = () => {
    switch (tone()) {
      case 'green':
        return 'bg-emerald-500'
      case 'blue':
        return 'bg-sky-500'
      case 'amber':
        return 'bg-amber-500'
      case 'red':
        return 'bg-[#e82127]'
      default:
        return 'bg-zinc-400 dark:bg-zinc-500'
    }
  }
  const wrap = () => {
    switch (tone()) {
      case 'green':
        return 'bg-emerald-500/10 text-emerald-700 ring-emerald-600/25 dark:text-emerald-300 dark:ring-emerald-500/25'
      case 'blue':
        return 'bg-sky-500/10 text-sky-700 ring-sky-600/25 dark:text-sky-300 dark:ring-sky-500/25'
      case 'amber':
        return 'bg-amber-500/10 text-amber-700 ring-amber-600/25 dark:text-amber-300 dark:ring-amber-500/25'
      case 'red':
        return 'bg-[#e82127]/10 text-red-700 ring-red-600/25 dark:text-red-300 dark:ring-red-500/25'
      default:
        return 'bg-zinc-900/[0.05] text-zinc-600 ring-zinc-900/10 dark:bg-white/[0.06] dark:text-zinc-300 dark:ring-white/10'
    }
  }
  return (
    <span class={`inline-flex items-center gap-1.5 rounded-full px-2.5 py-1 text-xs font-medium ring-1 ring-inset ${wrap()}`}>
      <span class={`h-1.5 w-1.5 rounded-full ${dot()} ${props.pulse ? 'animate-[pulse-dot_1.6s_ease-in-out_infinite]' : ''}`} />
      {props.children}
    </span>
  )
}

/* ---------- Feedback ---------- */

export function Alert(props: { tone?: 'error' | 'success' | 'info'; children: JSX.Element }) {
  const isError = () => props.tone === 'error'
  return (
    <div
      role={isError() ? 'alert' : 'status'}
      class={`rounded-xl border px-3.5 py-2.5 text-sm ${
        props.tone === 'error'
          ? 'border-red-500/25 bg-red-500/[0.07] text-red-700 dark:text-red-200'
          : props.tone === 'success'
            ? 'border-emerald-600/25 bg-emerald-500/[0.08] text-emerald-700 dark:text-emerald-200'
            : 'border-zinc-200 bg-zinc-100 text-zinc-600 dark:border-white/10 dark:bg-white/[0.04] dark:text-zinc-300'
      }`}
    >
      {props.children}
    </div>
  )
}

export function Spinner() {
  return (
    <div class="flex items-center justify-center gap-2 py-10 text-sm text-zinc-500">
      <span class="h-4 w-4 animate-spin rounded-full border-2 border-zinc-300 border-t-[#e82127] dark:border-zinc-600 dark:border-t-[#e82127]" />
      Loading…
    </div>
  )
}

export function SkeletonCard() {
  return (
    <div class="animate-pulse rounded-2xl border border-zinc-200 bg-white p-5 dark:border-white/[0.07] dark:bg-[#101013]/90">
      <div class="mb-3 flex items-center justify-between">
        <div class="h-5 w-32 rounded bg-zinc-200 dark:bg-white/10" />
        <div class="h-5 w-16 rounded-full bg-zinc-200 dark:bg-white/10" />
      </div>
      <div class="mb-3 h-44 rounded-xl bg-zinc-100 dark:bg-white/[0.06]" />
      <div class="grid grid-cols-2 gap-2">
        <div class="h-12 rounded-lg bg-zinc-100 dark:bg-white/[0.06]" />
        <div class="h-12 rounded-lg bg-zinc-100 dark:bg-white/[0.06]" />
        <div class="h-12 rounded-lg bg-zinc-100 dark:bg-white/[0.06]" />
        <div class="h-12 rounded-lg bg-zinc-100 dark:bg-white/[0.06]" />
      </div>
    </div>
  )
}

export function EmptyState(props: { title: string; hint?: string; action?: JSX.Element }) {
  return (
    <div class="rounded-2xl border border-dashed border-zinc-300 bg-zinc-50 px-6 py-12 text-center dark:border-white/10 dark:bg-white/[0.02]">
      <p class="text-sm font-semibold text-zinc-700 dark:text-zinc-200">{props.title}</p>
      <Show when={props.hint}>
        <p class="mx-auto mt-1 max-w-sm text-sm text-zinc-500">{props.hint}</p>
      </Show>
      <Show when={props.action}>
        <div class="mt-4 flex justify-center">{props.action}</div>
      </Show>
    </div>
  )
}

/* ---------- Forms ---------- */

export function FormField(props: {
  label: string
  type?: string
  value: string
  onInput: (v: string) => void
  placeholder?: string
  hint?: string
  error?: string | null
}) {
  return (
    <label class="block">
      <span class="mb-1.5 block text-[13px] font-medium text-zinc-700 dark:text-zinc-300">{props.label}</span>
      <input
        type={props.type ?? 'text'}
        value={props.value}
        onInput={(e) => props.onInput(e.currentTarget.value)}
        placeholder={props.placeholder}
        class="w-full rounded-xl border border-zinc-300 bg-white px-3 py-2 text-sm text-zinc-900 placeholder:text-zinc-400 transition-colors focus:border-[#e82127]/60 focus:outline-none focus:ring-2 focus:ring-[#e82127]/20 dark:border-white/10 dark:bg-white/[0.04] dark:text-zinc-100 dark:placeholder:text-zinc-600"
      />
      <Show when={props.hint}>
        <span class="mt-1 block text-xs text-zinc-500">{props.hint}</span>
      </Show>
      <Show when={props.error}>
        <span class="mt-1 block text-xs text-red-600 dark:text-red-400">{props.error}</span>
      </Show>
    </label>
  )
}

export function Select(props: {
  label: string
  value: string
  onChange: (v: string) => void
  options: Array<{ value: string; label?: string }>
  hint?: string
}) {
  return (
    <label class="block">
      <span class="mb-1.5 block text-[13px] font-medium text-zinc-700 dark:text-zinc-300">{props.label}</span>
      <select
        value={props.value}
        onChange={(e) => props.onChange(e.currentTarget.value)}
        class="w-full appearance-none rounded-xl border border-zinc-300 bg-white px-3 py-2 text-sm text-zinc-900 transition-colors focus:border-[#e82127]/60 focus:outline-none focus:ring-2 focus:ring-[#e82127]/20 dark:border-white/10 dark:bg-white/[0.04] dark:text-zinc-100 [&>option]:bg-white dark:[&>option]:bg-zinc-900"
      >
        <For each={props.options}>
          {(o) => <option value={o.value}>{o.label ?? o.value}</option>}
        </For>
      </select>
      <Show when={props.hint}>
        <span class="mt-1 block text-xs text-zinc-500">{props.hint}</span>
      </Show>
    </label>
  )
}

export function Check(props: { label: string; hint?: string; checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={props.checked}
      onClick={() => props.onChange(!props.checked)}
      class="flex w-full items-center justify-between gap-3 rounded-xl border border-zinc-200 bg-white px-3 py-2.5 text-left transition-colors hover:border-zinc-300 dark:border-white/[0.07] dark:bg-white/[0.03] dark:hover:border-white/[0.14]"
    >
      <span>
        <span class="block text-sm font-medium text-zinc-800 dark:text-zinc-200">{props.label}</span>
        <Show when={props.hint}>
          <span class="block text-xs text-zinc-500">{props.hint}</span>
        </Show>
      </span>
      <span
        class={`relative h-6 w-11 shrink-0 rounded-full transition-colors ${props.checked ? 'bg-[#e82127]' : 'bg-zinc-300 dark:bg-white/10'}`}
      >
        <span
          class={`absolute top-0.5 h-5 w-5 rounded-full bg-white shadow transition-all ${props.checked ? 'left-[22px]' : 'left-0.5'}`}
        />
      </span>
    </button>
  )
}

export function BackLink(props: { href: string; children: JSX.Element }) {
  return (
    <a href={props.href} class="mb-3 inline-flex items-center gap-1 text-sm text-zinc-500 transition-colors hover:text-zinc-800 dark:hover:text-zinc-200">
      <Icon d={I.back} class="h-3.5 w-3.5" />
      {props.children}
    </a>
  )
}
