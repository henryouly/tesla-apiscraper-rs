/**
 * Strict numeric parsing for form inputs. Unlike `parseFloat` (which
 * accepts valid prefixes like "37abc" -> 37) or unary plus, the entire
 * trimmed value must be a finite number — otherwise `null`, and the
 * caller shows an error instead of submitting a silently truncated value.
 */
export function parseNumber(text: string): number | null {
  const trimmed = text.trim()
  if (!trimmed) return null
  const n = Number(trimmed)
  return Number.isFinite(n) ? n : null
}

/** Strict non-negative integer parsing (u64-backed API fields). */
export function parseNonNegativeInt(text: string): number | null {
  const n = parseNumber(text)
  return n != null && Number.isInteger(n) && n >= 0 ? n : null
}
