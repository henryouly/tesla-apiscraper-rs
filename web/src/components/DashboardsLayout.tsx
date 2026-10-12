import { useLocation } from '@solidjs/router'
import { Show, createContext, createEffect, createResource, createSignal, useContext, type ParentProps } from 'solid-js'
import { api, type VehicleDiscovery } from '../lib/api'
import { defaultRange, type Range } from '../lib/dashboards'
import { DASHBOARD_TABS, DashboardsNav } from './DashboardsNav'
import { TimeRangePicker } from './TimeRangePicker'
import { Alert, Card, EmptyState, PageHeader, Select, Spinner } from './ui'

export interface DashboardScope {
  vin: () => string | null
  setVin: (v: string) => void
  vehicles: () => VehicleDiscovery[] | undefined
  vehiclesLoading: () => boolean
  vehiclesError: unknown
  range: () => Range
  setRange: (r: Range) => void
}

const DashboardScopeContext = createContext<DashboardScope | null>(null)

export function useDashboardScope(): DashboardScope {
  const scope = useContext(DashboardScopeContext)
  if (scope == null) throw new Error('useDashboardScope must be used inside DashboardsLayout')
  return scope
}

const NO_VEHICLES_HINT = 'Once a car is linked to this Tesla account and reports telemetry, it will appear here.'

function DashboardsBody(props: ParentProps) {
  const scope = useDashboardScope()
  const loc = useLocation()
  // Prefix-aware so detail routes (e.g. /dashboards/charges/:id, which has
  // no tab of its own) still show the parent tab's header. Only the layout
  // header is prefix-aware; the sidebar nav keeps exact matching.
  const active = () =>
    DASHBOARD_TABS.find((t) => loc.pathname === t.href || loc.pathname.startsWith(`${t.href}/`)) ??
    DASHBOARD_TABS.find((t) => t.href === '/dashboards/overview') ??
    DASHBOARD_TABS[0]
  const vinOptions = () =>
    scope.vehicles()?.map((v) => ({ value: v.vin, label: v.display_name || v.vin })) ?? []
  const hasVehicles = () => (scope.vehicles()?.length ?? 0) > 0

  return (
    <div>
      <PageHeader eyebrow="Dashboards" title={active().title} hint={active().description} />
      <Show when={scope.vehiclesLoading()}>
        <Spinner />
      </Show>
      <Show when={scope.vehiclesError}>
        <Alert tone="error">
          {scope.vehiclesError instanceof Error ? scope.vehiclesError.message : 'Failed to load vehicles.'}
        </Alert>
      </Show>
      <Show when={!scope.vehiclesLoading() && !scope.vehiclesError && !hasVehicles()}>
        <EmptyState title="No vehicles found" hint={NO_VEHICLES_HINT} />
      </Show>
      <Show when={!scope.vehiclesLoading() && !scope.vehiclesError && hasVehicles()}>
        <Card class="relative z-30 mb-4 p-5">
          <div class="grid gap-4 sm:grid-cols-2 sm:items-end">
            <Select label="Vehicle" value={scope.vin() ?? ''} onChange={scope.setVin} options={vinOptions()} />
            <TimeRangePicker value={scope.range()} onChange={scope.setRange} />
          </div>
        </Card>
        <div class="flex flex-col gap-6 md:flex-row">
          <DashboardsNav />
          <div class="min-w-0 flex-1">
            {props.children}
          </div>
        </div>
      </Show>
    </div>
  )
}

export function DashboardsLayout(props: ParentProps) {
  const [vin, setVin] = createSignal<string | null>(null)
  const [range, setRange] = createSignal<Range>(defaultRange(7))
  const [vehiclesResource] = createResource(() => api.vehicles())

  // Default to the first known vehicle once discovery resolves.
  createEffect(() => {
    if (vin() == null) {
      const first = vehiclesResource()?.vehicles[0]
      if (first) setVin(first.vin)
    }
  })

  const scope: DashboardScope = {
    vin,
    setVin: (v: string) => setVin(v),
    vehicles: () => vehiclesResource()?.vehicles,
    vehiclesLoading: () => vehiclesResource.loading,
    get vehiclesError(): unknown {
      return vehiclesResource.error
    },
    range,
    setRange: (r: Range) => setRange(r),
  }

  return (
    <DashboardScopeContext.Provider value={scope}>
      <DashboardsBody>{props.children}</DashboardsBody>
    </DashboardScopeContext.Provider>
  )
}
