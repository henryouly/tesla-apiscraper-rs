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

export interface SoftwareUpdate {
  t: number
  version_before: string | null
  version_after: string | null
  install_start: string | null
  install_end: string | null
  status: string | null
}

export interface DashboardsUpdates {
  vin: string
  count: number
  updates: Array<SoftwareUpdate>
}

export interface UpdatesParams {
  vin: string
  from: string
  to: string
}

export function fetchUpdates(params: UpdatesParams): Promise<DashboardsUpdates> {
  return api.dashboardsUpdates(params)
}

export interface DashboardsMileage {
  vin: string
  points: Array<LevelPoint>
}

export interface MileageParams {
  vin: string
  from: string
  to: string
  unit: DistanceUnit
}

export function fetchMileage(params: MileageParams): Promise<DashboardsMileage> {
  return api.dashboardsMileage(params)
}

export interface ProjectedPoint {
  t: number
  ideal: number | null
  rated: number | null
}

export interface DashboardsProjectedRange {
  vin: string
  projected: Array<ProjectedPoint>
  outside_temp: Array<LevelPoint>
}

export interface ProjectedRangeParams {
  vin: string
  from: string
  to: string
  unit: DistanceUnit
}

export function fetchProjectedRange(params: ProjectedRangeParams): Promise<DashboardsProjectedRange> {
  return api.dashboardsProjectedRange(params)
}
