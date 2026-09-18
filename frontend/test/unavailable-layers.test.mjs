import assert from 'node:assert/strict'
import test from 'node:test'
import { unavailableLayersSentence } from '../src/lib/unavailable-layers.ts'

test('the popup names every layer the level lacks, and says nothing when none is missing', () => {
  assert.equal(unavailableLayersSentence(undefined), null)
  assert.equal(unavailableLayersSentence([]), null)
  assert.equal(
    unavailableLayersSentence(['aircraft']),
    'Aircraft data is unavailable here; the level shown does not include it.',
  )
  assert.equal(
    unavailableLayersSentence(['aircraft', 'leisure', 'ships']),
    'Aircraft, sports ground and ship data is unavailable here; the level shown does not include it.',
  )
})
