import type { JSX, ParentProps } from 'solid-js'

export function Button(props: {
  type?: 'button' | 'submit'
  disabled?: boolean
  variant?: 'primary' | 'ghost' | 'danger'
  onClick?: () => void
  children: JSX.Element
}) {
  const cls = () =>
    props.variant === 'ghost'
      ? 'border border-gray-300 dark:border-gray-700 hover:bg-gray-100 dark:hover:bg-gray-800'
      : props.variant === 'danger'
        ? 'bg-red-600 text-white hover:bg-red-700'
        : 'bg-blue-600 text-white hover:bg-blue-700'
  return (
    <button
      type={props.type ?? 'button'}
      disabled={props.disabled}
      onClick={() => props.onClick?.()}
      class={`rounded px-3 py-1.5 text-sm font-medium disabled:opacity-50 ${cls()}`}
    >
      {props.children}
    </button>
  )
}

export function Card(props: ParentProps<{ class?: string }>) {
  return (
    <div
      class={`rounded-lg border border-gray-200 bg-white p-4 shadow-sm dark:border-gray-800 dark:bg-gray-900 ${props.class ?? ''}`}
    >
      {props.children}
    </div>
  )
}

export function FormField(props: {
  label: string
  type?: string
  value: string
  onInput: (v: string) => void
  placeholder?: string
}) {
  return (
    <label class="block">
      <span class="mb-1 block text-sm font-medium">{props.label}</span>
      <input
        type={props.type ?? 'text'}
        value={props.value}
        onInput={(e) => props.onInput(e.currentTarget.value)}
        placeholder={props.placeholder}
        class="w-full rounded border border-gray-300 bg-white px-3 py-2 text-sm dark:border-gray-700 dark:bg-gray-800"
      />
    </label>
  )
}

export function Spinner() {
  return <div class="py-8 text-center text-sm text-gray-500">Loading…</div>
}
