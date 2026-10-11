import { api } from './api'

// Shared dashboard client + helpers (Stage 0 foundations).

export interface TimePoint {
  /** Epoch millis (uPlot takes numbers, not strings). */
  t: number
  v: number | null
}

export interface Range {
  /** RFC3339 timestamps. */
  from: string
  to: string
}

/** Distance unit, matching the Grafana `length_unit` semantics. */
export type DistanceUnit = 'km' | 'mi'

export interface BatteryPoint {
  t: number
  level: number | null
  usable: number | null
}

export interface LevelPoint {
  t: number
  value: number | null
}

export interface DashboardsOverview {
  vin: string
  battery: Array<BatteryPoint>
  ideal_range: Array<LevelPoint>
}

export interface OverviewParams {
  vin: string
  from: string
  to: string
  unit: DistanceUnit
}

/** Default `to` = now, `from` = `days` ago. */
export function defaultRange(days = 7): Range {
  const to = new Date()
  const from = new Date(to.getTime() - days * 24 * 60 * 60 * 1000)
  return { from: from.toISOString(), to: to.toISOString() }
}

export function toEpochMs(iso: string): number {
  return Date.parse(iso)
}

export function fetchOverview(params: OverviewParams): Promise<DashboardsOverview> {
  return api.dashboardsOverview(params)
}

export interface StatePoint {
  t: number
  state: string
}

export interface DashboardsStates {
  vin: string
  current: string | null
  timeline: Array<StatePoint>
  changes: number
}

export interface StatesParams {
  vin: string
  from: string
  to: string
}

export function fetchStates(params: StatesParams): Promise<DashboardsStates> {
  return api.dashboardsStates(params)
}
