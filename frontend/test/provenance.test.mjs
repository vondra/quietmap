import assert from 'node:assert/strict'
import test from 'node:test'

import {
  railTrainSourceLine,
  roadSourceDescription,
} from '../src/components/noise/provenance.ts'

function dataset(tier, overrides = {}) {
  return {
    tier,
    name: 'Example dataset',
    year: 2024,
    license: 'CC-BY-4.0',
    url: null,
    ...overrides,
  }
}

for (const tier of [
  'city-measured',
  'national-measured',
  'continental-measured',
  'global-measured',
]) {
  test(`road ${tier} is described as an external input without overclaiming`, () => {
    assert.equal(
      roadSourceDescription('matched_external', dataset(tier), 'primary'),
      'Source: Example dataset (2024) · CC-BY-4.0\n' +
        '  (external AADT input per OSM way)',
    )
  })
}

test('national road proxy is clearly described as an estimate', () => {
  const description = roadSourceDescription(
    'matched_external',
    dataset('national-proxy'),
    'primary',
  )
  assert.equal(
    description,
    'Source: Example dataset (2024) · CC-BY-4.0\n' +
      '  (estimated AADT, not a direct measurement)',
  )
  assert.doesNotMatch(description, /\bmeasured\b/i)
})

test('national rail proxy is clearly described as an estimate', () => {
  const description = railTrainSourceLine(
    'arrow',
    dataset('national-proxy'),
    'rail',
  )
  assert.equal(
    description,
    'Example dataset (2024) · CC-BY-4.0\n' +
      '  (estimated trains/day, not a direct measurement)',
  )
  assert.doesNotMatch(description, /\bmeasured\b/i)
})

test('heuristic and baseline rail counts are explicitly model-derived', () => {
  assert.equal(
    railTrainSourceLine('arrow', dataset('heuristic'), 'rail'),
    'Example dataset (2024) · CC-BY-4.0\n' +
      '  (estimated trains/day, not a direct measurement)',
  )
  assert.equal(
    railTrainSourceLine('arrow', dataset('baseline'), 'rail'),
    'Example dataset (2024) · CC-BY-4.0\n' +
      '  (model-derived trains/day baseline, not an observed count)',
  )
})

test('authoritative rail input stays neutral about observed versus scheduled counts', () => {
  assert.equal(
    railTrainSourceLine('arrow', dataset('national-measured'), 'rail'),
    'Example dataset (2024) · CC-BY-4.0\n' +
      '  (external trains/day input per OSM way)',
  )
})

test('a missing tier fails conservatively during a rolling deploy', () => {
  const legacyDataset = dataset(undefined)
  const description = roadSourceDescription('matched_external', legacyDataset, 'primary')
  assert.match(description, /measurement status unavailable/)
  assert.doesNotMatch(description, /\bmeasured\b/i)
  assert.doesNotMatch(description, /estimated AADT/i)
})

test('an inconsistent none tier stays neutral', () => {
  const description = railTrainSourceLine('arrow', dataset('none'), 'rail')
  assert.match(description, /measurement status unavailable/)
  assert.doesNotMatch(description, /\bmeasured\b/i)
})

test('missing dataset metadata never falls through to a CNOSSOS default label', () => {
  const road = roadSourceDescription('matched_external', null, 'primary')
  const rail = railTrainSourceLine('arrow', null, 'rail')
  assert.match(road, /external road-traffic input/i)
  assert.match(road, /metadata and measurement status unavailable/i)
  assert.doesNotMatch(road, /CNOSSOS/i)
  assert.match(rail, /external train-count input/i)
  assert.match(rail, /metadata and measurement status unavailable/i)
  assert.doesNotMatch(rail, /CNOSSOS/i)
})

test('speed-only baseline provenance does not override the class-default traffic source', () => {
  // Regression: taper rows can carry baseline provenance while raw AADT stays
  // zero, so the engine reports default_by_class and uses the class default.
  const description = roadSourceDescription(
    'default_by_class',
    dataset('baseline', { name: 'Transition taper' }),
    'secondary',
  )
  assert.equal(
    description,
    'Source: Transition taper (2024) · CC-BY-4.0 — secondary class\n' +
      '  (class-default traffic count; listed source may apply to speed only)',
  )
  assert.doesNotMatch(description, /model-derived/i)
  assert.doesNotMatch(description, /not a class-default/i)
  assert.doesNotMatch(description, /no enrichment data/i)
})

test('heuristic and CNOSSOS fallback wording remains explicit', () => {
  assert.equal(
    roadSourceDescription('estimated_service_tree', dataset('heuristic'), 'residential'),
    'Source: Example dataset (2024) · CC-BY-4.0 — residential class\n' +
      '  (estimated AADT, not a direct measurement)',
  )
  assert.equal(
    railTrainSourceLine('default_by_type', null, 'tram'),
    'CNOSSOS Annex IV default — tram\n  (no enrichment data)',
  )
})
