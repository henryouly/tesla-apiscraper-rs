import { createContext, createResource, useContext, type ParentProps } from 'solid-js'
import { api } from './api'

// Tesla Owner API distances are miles and speeds are mph, regardless of
// car display settings. Card formatters convert per unit_length.
const MI_TO_KM = 1.60934

const UnitsContext = createContext<{
  unitLength: () => string
  formatRange: (miles: number | null | undefined) => string
  formatSpeed: (mph: number | null | undefined) => string
}>()

export function UnitsProvider(props: ParentProps) {
  const [settings] = createResource(() => api.settings().catch(() => null))
  // Fail open to raw API values (miles) when settings are unreachable.
  const unitLength = () => settings()?.settings.global.unit_length ?? 'mi'
  const formatRange = (miles: number | null | undefined) => {
    if (miles == null) return '—'
    return unitLength() === 'km'
      ? `${(miles * MI_TO_KM).toFixed(0)} km`
      : `${miles.toFixed(0)} mi`
  }
  const formatSpeed = (mph: number | null | undefined) => {
    if (mph == null) return '—'
    return unitLength() === 'km' ? `${(mph * MI_TO_KM).toFixed(0)} km/h` : `${mph.toFixed(0)} mph`
  }
  return (
    <UnitsContext.Provider value={{ unitLength, formatRange, formatSpeed }}>
      {props.children}
    </UnitsContext.Provider>
  )
}

export function useUnits() {
  const ctx = useContext(UnitsContext)
  if (!ctx) throw new Error('useUnits outside UnitsProvider')
  return ctx
}
