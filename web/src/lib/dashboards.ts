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

export interface DashboardsEfficiency {
  vin: string
  avg_consumption_wh_per_km: number | null
  logged_distance: number
  per_drive: Array<LevelPoint>
}

export interface EfficiencyParams {
  vin: string
  from: string
  to: string
  unit: DistanceUnit
}

export function fetchEfficiency(params: EfficiencyParams): Promise<DashboardsEfficiency> {
  return api.dashboardsEfficiency(params)
}

export interface TopDestination {
  place: string
  visits: number
}

export interface VisitedRow {
  t: number
  address: string | null
  geofence: string | null
  distance: number | null
}

export interface DashboardsLocations {
  vin: string
  top: Array<TopDestination>
  last_visited: Array<VisitedRow>
  drives_logged: number
}

export interface LocationsParams {
  vin: string
  from: string
  to: string
  unit: DistanceUnit
}

export function fetchLocations(params: LocationsParams): Promise<DashboardsLocations> {
  return api.dashboardsLocations(params)
}

export interface DashboardsDriveStats {
  vin: string
  drives: number
  distance: number
  energy_kwh: number | null
  median_drive: number | null
  max_speed: number | null
  speeds: Array<number>
  top_destinations: Array<TopDestination>
}

export interface DriveStatsParams {
  vin: string
  from: string
  to: string
  unit: DistanceUnit
}

export function fetchDriveStats(params: DriveStatsParams): Promise<DashboardsDriveStats> {
  return api.dashboardsDriveStats(params)
}

export interface ChargeSessionRow {
  t: number
  charge_id: string
  location: string | null
  energy_kwh: number | null
  duration_min: number | null
  cost: number | null
  start_pct: number | null
  end_pct: number | null
}

export interface SessionPower {
  charge_id: string
  points: Array<LevelPoint>
}

export interface DashboardsCharges {
  vin: string
  count: number
  energy_kwh: number
  total_cost: number
  avg_cost_per_kwh: number | null
  sessions: Array<ChargeSessionRow>
  energy_per_day: Array<LevelPoint>
  power_by_session: Array<SessionPower>
}

export interface ChargesParams {
  vin: string
  from: string
  to: string
}

export function fetchCharges(params: ChargesParams): Promise<DashboardsCharges> {
  return api.dashboardsCharges(params)
}

export interface SocDeltaPoint {
  t: number
  start: number | null
  end: number | null
}

export interface TopStation {
  station: string
  energy_kwh: number
  sessions: number
}

export interface DashboardsChargingStats {
  vin: string
  count: number
  energy_kwh: number
  total_cost: number
  avg_cost_per_kwh: number | null
  soc_delta: Array<SocDeltaPoint>
  top_stations: Array<TopStation>
}

export interface ChargingStatsParams {
  vin: string
  from: string
  to: string
}

export function fetchChargingStats(params: ChargingStatsParams): Promise<DashboardsChargingStats> {
  return api.dashboardsChargingStats(params)
}

export interface ChargeCurvePoint {
  t: number
  battery: number | null
  power_kw: number | null
  added_kwh: number | null
}

export interface DashboardsChargeDetails {
  charge_id: string
  cost: number | null
  duration_s: number | null
  geofence: string | null
  energy_kwh: number | null
  lat: number | null
  lng: number | null
  soc_added: number | null
  curve: Array<ChargeCurvePoint>
}

export function fetchChargeDetails(chargeId: string): Promise<DashboardsChargeDetails> {
  return api.dashboardsChargeDetails(chargeId)
}
