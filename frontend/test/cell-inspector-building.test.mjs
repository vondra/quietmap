import assert from 'node:assert/strict'
import test from 'node:test'
import {
  formatBuildingAt,
  parseBuildingAtResponse,
} from '../src/components/cell-inspector-building.ts'

test('a null vector lookup stays a visible no-building result', () => {
  assert.deepEqual(parseBuildingAtResponse(null), { kind: 'none' })
  assert.equal(formatBuildingAt({ status: 'ready', result: null }), 'none')
})

test('the unavailable marker does not become a no-building result', () => {
  assert.deepEqual(
    parseBuildingAtResponse({ status: 'unavailable' }),
    { kind: 'unavailable' },
  )
  assert.equal(formatBuildingAt({ status: 'failed' }), 'unavailable')
})
