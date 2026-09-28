import { createContext, createEffect, createResource, useContext, type ParentProps } from 'solid-js'
import { api } from './api'
import { useAuth } from './auth'

// Tesla Owner API distances are miles and speeds are mph, regardless of
// car display settings. Card formatters convert per unit_length.
const MI_TO_KM = 1.60934

const UnitsContext = createContext<{
  unitLength: () => string
  unitTemperature: () => string
  preferredRange: () => string
  formatRange: (rated: number | null | undefined, ideal?: number | null) => string
  formatDistance: (miles: number | null | undefined) => string
  formatSpeed: (mph: number | null | undefined) => string
  formatTemp: (celsius: number | null | undefined) => string
  formatDurationHours: (hours: number | null | undefined) => string
  formatPowerKw: (kw: number | null | undefined) => string
  formatEnergyKwh: (kwh: number | null | undefined) => string
  refresh: () => void
}>()

export function UnitsProvider(props: ParentProps) {
  const auth = useAuth()
  const [settings, { refetch }] = createResource(() => api.settings().catch(() => null))
  // The provider mounts above RequireAuth, so the initial fetch can 401
  // before sign-in: refetch once authentication arrives.
  createEffect(() => {
    if (auth.authenticated()) refetch()
  })
  // Fail open to raw API values (miles) when settings are unreachable.
  const unitLength = () => settings()?.settings.global.unit_length ?? 'mi'
  // API temperatures are Celsius; convert when the user prefers Fahrenheit.
  const unitTemperature = () => settings()?.settings.global.unit_temperature ?? 'C'
  // Rated unless the user prefers ideal (unknown values fall back to rated).
  const preferredRange = () => settings()?.settings.global.preferred_range ?? 'rated'
  const formatRange = (rated: number | null | undefined, ideal?: number | null) => {
    const miles = preferredRange() === 'ideal' && ideal != null ? ideal : rated
    if (miles == null) return '—'
    return unitLength() === 'km'
      ? `${(miles * MI_TO_KM).toFixed(0)} km`
      : `${miles.toFixed(0)} mi`
  }
  const formatDistance = (miles: number | null | undefined) => {
    if (miles == null) return '—'
    return unitLength() === 'km' ? `${(miles * MI_TO_KM).toFixed(0)} km` : `${miles.toFixed(0)} mi`
  }
  const formatSpeed = (mph: number | null | undefined) => {
    if (mph == null) return '—'
    return unitLength() === 'km' ? `${(mph * MI_TO_KM).toFixed(0)} km/h` : `${mph.toFixed(0)} mph`
  }
  const formatTemp = (celsius: number | null | undefined) => {
    if (celsius == null) return '—'
    return unitTemperature() === 'F'
      ? `${((celsius * 9) / 5 + 32).toFixed(1)} °F`
      : `${celsius.toFixed(1)} °C`
  }
  const formatDurationHours = (hours: number | null | undefined) => {
    if (hours == null) return '—'
    const totalMin = Math.round(hours * 60)
    const h = Math.floor(totalMin / 60)
    const m = totalMin % 60
    return h > 0 ? `${h}h ${m}m` : `${m}m`
  }
  const formatPowerKw = (kw: number | null | undefined) => {
    if (kw == null) return '—'
    return `${kw} kW`
  }
  const formatEnergyKwh = (kwh: number | null | undefined) => {
    if (kwh == null) return '—'
    return `${kwh} kWh`
  }
  return (
    <UnitsContext.Provider
      value={{
        unitLength,
        unitTemperature,
        preferredRange,
        formatRange,
        formatDistance,
        formatSpeed,
        formatTemp,
        formatDurationHours,
        formatPowerKw,
        formatEnergyKwh,
        refresh: refetch,
      }}
    >
      {props.children}
    </UnitsContext.Provider>
  )
}

export function useUnits() {
  const ctx = useContext(UnitsContext)
  if (!ctx) throw new Error('useUnits outside UnitsProvider')
  return ctx
}
