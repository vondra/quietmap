/** Holdout coordinates cannot reach the training feature reader, even with forged square labels. */
import assert from 'node:assert/strict'
import { test } from 'node:test'
import { trainingSquareFeatures } from './local-street-features.js'

test('training extraction rejects real and relabelled holdouts before reading prepared files', () => {
  const barcelona = { id: 'holdout', lat: 41.3874, lon: 2.1686, x: 259, y: 191 }
  assert.throws(() => trainingSquareFeatures('/nonexistent', [barcelona]), /not a training point/)
  assert.throws(() => trainingSquareFeatures('/nonexistent', [{ ...barcelona, x: 276, y: 173 }]), /not a training point/)
})
