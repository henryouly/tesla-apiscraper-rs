import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import {
  computeInstallDurationSec,
  formatUpdateDuration,
  formatUpdateTimestamp,
  updateStatusBadge,
} from '../src/lib/updates.ts'

describe('computeInstallDurationSec', () => {
  it('computes non-negative seconds between valid RFC3339 timestamps', () => {
    assert.equal(computeInstallDurationSec('2024-11-14T10:00:00Z', '2024-11-14T10:30:00Z'), 1800)
    assert.equal(computeInstallDurationSec('2024-11-14T10:00:00Z', '2024-11-14T10:00:45Z'), 45)
    assert.equal(computeInstallDurationSec('2024-11-14T10:00:00Z', '2024-11-14T10:00:00Z'), 0)
  })

  it('returns null for missing, invalid, or inverted timestamps', () => {
    assert.equal(computeInstallDurationSec(null, '2024-11-14T10:30:00Z'), null)
    assert.equal(computeInstallDurationSec('2024-11-14T10:00:00Z', null), null)
    assert.equal(computeInstallDurationSec('bad', '2024-11-14T10:30:00Z'), null)
    assert.equal(computeInstallDurationSec('2024-11-14T10:30:00Z', '2024-11-14T10:00:00Z'), null)
  })
})

describe('formatUpdateDuration', () => {
  it('formats seconds, minutes, and hours cleanly', () => {
    assert.equal(formatUpdateDuration(null), '—')
    assert.equal(formatUpdateDuration(-5), '—')
    assert.equal(formatUpdateDuration(45), '45s')
    assert.equal(formatUpdateDuration(1800), '30 min')
    assert.equal(formatUpdateDuration(3600), '1h')
    assert.equal(formatUpdateDuration(4500), '1h 15m')
  })
})

describe('formatUpdateTimestamp', () => {
  it('returns dash for empty input and preserves unparseable strings', () => {
    assert.equal(formatUpdateTimestamp(null), '—')
    assert.equal(formatUpdateTimestamp(''), '—')
    assert.equal(formatUpdateTimestamp('not-a-date'), 'not-a-date')
    const formatted = formatUpdateTimestamp('2024-11-14T10:00:00Z')
    assert.ok(formatted.length > 0 && formatted !== '—')
  })
})

describe('updateStatusBadge', () => {
  it('maps status and abandoned flag to expected tones', () => {
    assert.deepEqual(updateStatusBadge('completed', false), { label: 'completed', tone: 'green' })
    assert.deepEqual(updateStatusBadge('installed', null), { label: 'installed', tone: 'green' })
    assert.deepEqual(updateStatusBadge('installing', false), { label: 'installing', tone: 'amber' })
    assert.deepEqual(updateStatusBadge('cancelled', false), { label: 'cancelled', tone: 'red' })
    assert.deepEqual(updateStatusBadge('installing', true), { label: 'abandoned', tone: 'red' })
    assert.deepEqual(updateStatusBadge(null, null), { label: 'unknown', tone: 'gray' })
  })
})
