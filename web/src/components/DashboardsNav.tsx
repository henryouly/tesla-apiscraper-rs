import { A, useLocation } from '@solidjs/router'
import { For, Show } from 'solid-js'
import { Pill } from './ui'

export interface DashboardTab {
  href: string
  label: string
  ready: boolean
  title: string
  description: string
}

export const DASHBOARD_TABS: Array<DashboardTab> = [
  {
    href: '/dashboards/overview',
    label: 'Overview',
    ready: true,
    title: 'Overview',
    description: 'Charge level and ideal range for the last 7 days.',
  },
  {
    href: '/dashboards/states',
    label: 'States',
    ready: true,
    title: 'States',
    description: 'Vehicle state history and transitions over time.',
  },
  {
    href: '/dashboards/updates',
    label: 'Updates',
    ready: true,
    title: 'Updates',
    description: 'Software update history for this vehicle.',
  },
  {
    href: '/dashboards/mileage',
    label: 'Mileage',
    ready: true,
    title: 'Mileage',
    description: 'Odometer history for this vehicle.',
  },
  {
    href: '/dashboards/projected-range',
    label: 'Projected Range',
    ready: true,
    title: 'Projected Range',
    description: 'Projected 100% range and outside temperature.',
  },
  {
    href: '/dashboards/efficiency',
    label: 'Efficiency',
    ready: true,
    title: 'Efficiency',
    description: 'Energy consumption and logged distance per drive.',
  },
  {
    href: '/dashboards/drives',
    label: 'Drives',
    ready: false,
    title: 'Drives',
    description: 'Trip history, distance, and efficiency for every drive.',
  },
  {
    href: '/dashboards/charges',
    label: 'Charges',
    ready: false,
    title: 'Charges',
    description: 'Charging sessions, energy added, and cost per charge.',
  },
]

function tabClass(active: boolean): string {
  const base = 'rounded-lg px-3 py-1.5 text-sm font-medium transition-colors'
  return active
    ? `${base} bg-zinc-900/[0.06] text-zinc-900 dark:bg-white/[0.08] dark:text-zinc-50`
    : `${base} text-zinc-500 hover:bg-zinc-900/[0.04] hover:text-zinc-900 dark:text-zinc-400 dark:hover:bg-white/[0.05] dark:hover:text-zinc-100`
}

function TabItem(props: { tab: DashboardTab }) {
  const loc = useLocation()
  const active = () => loc.pathname === props.tab.href
  return (
    <Show
      when={props.tab.ready}
      fallback={
        <span
          aria-disabled="true"
          class={`${tabClass(false)} inline-flex cursor-not-allowed items-center gap-2 whitespace-nowrap opacity-60`}
        >
          {props.tab.label}
          <Pill>Soon</Pill>
        </span>
      }
    >
      <A
        href={props.tab.href}
        aria-current={active() ? 'page' : undefined}
        class={`${tabClass(active())} inline-flex items-center gap-2 whitespace-nowrap`}
      >
        {props.tab.label}
      </A>
    </Show>
  )
}

export function DashboardsNav() {
  return (
    <nav aria-label="Dashboards">
      {/* Mobile: horizontal scrollable tab row */}
      <div class="flex gap-1 overflow-x-auto pb-1 md:hidden">
        <For each={DASHBOARD_TABS}>{(tab) => <TabItem tab={tab} />}</For>
      </div>
      {/* Desktop: vertical sidebar */}
      <div class="sticky top-20 hidden w-56 shrink-0 flex-col gap-1 self-start md:flex">
        <For each={DASHBOARD_TABS}>{(tab) => <TabItem tab={tab} />}</For>
      </div>
    </nav>
  )
}
