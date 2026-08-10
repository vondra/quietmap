/**
 * chain/manifest.ts — plan resolution for the `railway-europe*` step family
 * (2026-07-16 Phase 4: per-country fan-out, /gg review item 7's fix).
 *
 * Country scopes here are HAND-CONSTRUCTED `ResolvedScope` objects rather
 * than chain/scope.ts's real `parseScope`: `countryInScope`'s country-scope
 * branch is pure string comparison (`cc === scope.iso2.toLowerCase()`), so
 * building the scope directly keeps this file fully offline and sidesteps
 * `parseScope`'s CGAZ ADM0 bbox dependency (scripts/cache/ is gitignored —
 * cold on CI/a fresh checkout — the same reason lib/country-polygon.test.ts
 * is excluded from the fast gate in scripts/test.mjs). World scope goes
 * through the REAL `parseScope('world')`, which never touches CGAZ either
 * (immediate return, no bbox derivation needed for the whole-world case).
 *
 * Run: `cd pipeline && npx tsx --test chain/manifest.test.ts`
 */

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { buildPlan, type PlanStep } from './manifest.js'
import { parseScope, type ResolvedScope, type Bbox } from './scope.js'
import { FEEDS } from '../enrich-railway-europe.js'

function countryScope(iso2: string): ResolvedScope {
  return {
    kind: 'country',
    iso2,
    // Arbitrary bbox — countryInScope's country-scope branch never reads it
    // (string compare only); present so any bbox-fan-out step (e.g.
    // roads-built-up) still has something to iterate without crashing.
    bboxes: [[45, 5, 55, 20] as Bbox],
    label: `country:${iso2} [test]`,
    canonical: `country:${iso2}`,
  }
}

function railwayEuropeSteps(scope: ResolvedScope): PlanStep[] {
  return buildPlan(scope).steps.filter((s) => s.script === 'enrich-railway-europe.ts')
}

function obstacleHeightSteps(scope: ResolvedScope): PlanStep[] {
  return buildPlan(scope).steps.filter((s) => s.script === 'enrich-obstacle-heights.ts')
}

test('world scope: ONE railway-europe step, no --country, expectMinInputs DERIVED from the FEEDS registry (item 7: the hand-written 23 is dead)', () => {
  const steps = railwayEuropeSteps(parseScope('world'))
  assert.equal(steps.length, 1)
  assert.equal(steps[0].id, 'railway-europe')
  assert.equal(steps[0].country, null)
  assert.ok(!steps[0].args.includes('--country'), 'world mode must not pass --country — the enricher\'s own all-countries loop runs')
  assert.equal(steps[0].expectMinInputs, FEEDS.length, 'the floor tracks the registry — adding/removing a feed moves it with no manifest edit')
  assert.ok(FEEDS.length >= 20, 'sanity: the registry did not collapse (a near-empty FEEDS would make the floor vacuous)')
})

test('country:DE scope: exactly ONE railway-europe-de step, scoped with --country DE, expectMinInputs=1 (fixes /gg item 7)', () => {
  const steps = railwayEuropeSteps(countryScope('DE'))
  assert.equal(steps.length, 1, 'country:DE must select ONLY its own feed — not the whole 23-feed aggregate')
  assert.equal(steps[0].id, 'railway-europe-de')
  assert.equal(steps[0].country, 'de')
  assert.deepEqual(steps[0].args.slice(-2), ['--country', 'DE'])
  assert.equal(steps[0].expectMinInputs, 1)
})

test('country:FR scope: the ONE step floors expectMinInputs at 2 (fr + fr-idf), never the global 23', () => {
  const steps = railwayEuropeSteps(countryScope('FR'))
  assert.equal(steps.length, 1)
  assert.equal(steps[0].id, 'railway-europe-fr')
  assert.equal(steps[0].expectMinInputs, 2)
})

test('country:CZ scope: NO railway-europe step at all — CZ has no feed in this registry (runs its own bespoke enricher)', () => {
  const steps = railwayEuropeSteps(countryScope('CZ'))
  assert.equal(steps.length, 0)
})

test('a country scope never resolves a step for a DIFFERENT country — no accidental fan-out leakage', () => {
  const steps = railwayEuropeSteps(countryScope('DE'))
  assert.ok(!steps.some((s) => s.id === 'railway-europe-fr'))
  assert.ok(!steps.some((s) => s.id === 'railway-europe-at'))
  assert.ok(!steps.some((s) => s.id === 'railway-europe'), 'the world-mode step id must not also appear under a country scope')
})

test('country scope steps never carry the world-mode note about the enricher\'s own all-countries loop', () => {
  const steps = railwayEuropeSteps(countryScope('DE'))
  assert.match(steps[0].notes, /--country/)
  assert.doesNotMatch(steps[0].notes, /all-countries SEQUENTIAL loop/)
})

test('obstacle heights stays world-scoped and is never downgraded to a sync-safe cache skip', () => {
  const steps = obstacleHeightSteps(parseScope('world'))
  assert.equal(steps.length, 1)
  assert.equal(steps[0].id, 'obstacle-heights')
  assert.equal(steps[0].skipReason, null)
  assert.deepEqual(steps[0].args, ['--enrich-only'])
})

test('obstacle heights receives the exact bbox under a scoped chain run', () => {
  const scope: ResolvedScope = {
    kind: 'bbox',
    iso2: null,
    bboxes: [[49.7, 13.9, 50.4, 15.0]],
    label: 'bbox:test',
    canonical: 'bbox:49.7,13.9,50.4,15',
  }
  const steps = obstacleHeightSteps(scope)
  assert.equal(steps.length, 1)
  assert.equal(steps[0].skipReason, null)
  assert.deepEqual(steps[0].args, ['--enrich-only', '--bbox', '49.7,13.9,50.4,15'])
})
