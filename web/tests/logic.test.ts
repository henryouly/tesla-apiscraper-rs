import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import { previewCost } from '../src/lib/api.ts'
import { parseNonNegativeInt, parseNumber } from '../src/lib/number.ts'

describe('parseNumber', () => {
  it('parses valid finite numbers with surrounding whitespace', () => {
    assert.equal(parseNumber('0'), 0)
    assert.equal(parseNumber(' 37.7749 '), 37.7749)
    assert.equal(parseNumber('-122.4194'), -122.4194)
    assert.equal(parseNumber('1e3'), 1000)
  })

  it('rejects empty, non-finite, and trailing-junk strings', () => {
    for (const bad of ['', '   ', '37abc', 'abc', 'Infinity', '-Infinity', 'NaN', '12.3.4']) {
      assert.equal(parseNumber(bad), null, `expected null for ${JSON.stringify(bad)}`)
    }
  })
})

describe('parseNonNegativeInt', () => {
  it('accepts zero and positive integers', () => {
    assert.equal(parseNonNegativeInt('0'), 0)
    assert.equal(parseNonNegativeInt(' 21 '), 21)
    assert.equal(parseNonNegativeInt('1440'), 1440)
  })

  it('rejects negatives, floats, and invalid text', () => {
    for (const bad of ['-1', '-0.5', '15.5', '15abc', '', 'Infinity']) {
      assert.equal(parseNonNegativeInt(bad), null, `expected null for ${JSON.stringify(bad)}`)
    }
  })
})

describe('previewCost', () => {
  it('computes per_kwh cost with session fee rounded to 2 decimals', () => {
    assert.equal(previewCost('per_kwh', 40_000, 3600, 0.3, 1.5), 13.5)
    assert.equal(previewCost('per_kwh', 1000, 0, 0.1, 0.2), 0.3)
    assert.equal(previewCost('per_kwh', 33_333, 0, 0.33, 0), 11)
  })

  it('computes per_minute cost with session fee', () => {
    assert.equal(previewCost('per_minute', 0, 3600, 0.25, 0), 15)
    assert.equal(previewCost('per_minute', 50_000, 1800, 0.2, 1.25), 7.25)
  })
})
