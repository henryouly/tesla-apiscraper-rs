import { Card } from '../components/ui'

function Stub(props: { title: string }) {
  return (
    <Card>
      <h1 class="mb-2 text-xl font-bold">{props.title}</h1>
      <p class="text-sm text-gray-500">Coming in Phase 7.</p>
    </Card>
  )
}

export function Settings() {
  return <Stub title="Settings" />
}
export function CarSettings() {
  return <Stub title="Car settings" />
}
export function Geofences() {
  return <Stub title="Geofences" />
}
export function ChargeCost() {
  return <Stub title="Charge cost" />
}
