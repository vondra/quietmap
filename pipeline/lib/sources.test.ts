/**
 * Tests for the unified SOURCES registry + provenance-aware shouldOverwrite.
 * Run: `npx tsx --test pipeline/lib/sources.test.ts`
 */

import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  SOURCES,
  SOURCES_BY_ID,
  SOURCES_BY_KEY,
  UNSPECIFIED,
  PROVENANCE_RANK,
  shouldOverwrite,
  type Source,
  provenanceFromEntry,
  countryIsoForNationalSource,
} from './sources.js'
import { DATASETS } from './enrichment-datasets.js'

// ─── Registry derivation ───────────────────────────────────────────────────

test('SOURCES has one entry per DATASETS entry', () => {
  assert.strictEqual(SOURCES.length, DATASETS.length)
})

test('SOURCES_BY_ID stays in sync with SOURCES', () => {
  assert.strictEqual(SOURCES_BY_ID.size, SOURCES.length)
})

test('SOURCES_BY_KEY stays in sync with SOURCES', () => {
  assert.strictEqual(SOURCES_BY_KEY.size, SOURCES.length)
})

test('UNSPECIFIED is sentinel (id=0, provenance=none)', () => {
  assert.strictEqual(UNSPECIFIED.id, 0)
  assert.strictEqual(UNSPECIFIED.provenance, 'none')
})

test('country ownership comes from the registry key, including legacy city keys', () => {
  assert.strictEqual(countryIsoForNationalSource(20), 'CZ')
  assert.strictEqual(countryIsoForNationalSource(21), 'US')
  assert.strictEqual(countryIsoForNationalSource(9003), 'CZ')
  assert.strictEqual(countryIsoForNationalSource(9004), 'AT')
  assert.strictEqual(countryIsoForNationalSource(9005), 'CZ')
  assert.strictEqual(countryIsoForNationalSource(10), null)
  for (const source of SOURCES.filter(candidate => candidate.nationallyOwned)) {
    assert.match(countryIsoForNationalSource(source.id)!, /^[A-Z]{2}$/, source.key)
  }
})

// ─── Provenance mapping from priority ──────────────────────────────────────
// Real registry: 0=unspecified(0), 10=eu-city-traffic(70), 11=service-tree(10),
// 20=cz-rsd(80), 300=global-gppd(50), 9000=industrial-name-heuristic(10),
// 9001=global-overture(50), 9002=global-copernicus(50)

test('priority 90 → city-measured', () => {
  const city = DATASETS.find((dataset) => dataset.id === 9005)!
  assert.strictEqual(provenanceFromEntry(city), 'city-measured')
})

test('priority 80 → national-measured', () => {
  const rsd = SOURCES_BY_KEY.get('cz-rsd-scitani')
  assert.ok(rsd)
  assert.strictEqual(rsd.provenance, 'national-measured')
})

test('priority 70 → continental-measured', () => {
  const euCity = SOURCES_BY_KEY.get('eu-city-traffic')
  assert.ok(euCity)
  assert.strictEqual(euCity.provenance, 'continental-measured')
})

test('priority 50 GPPD → global-measured (per-facility observational)', () => {
  const gppd = SOURCES_BY_KEY.get('global-gppd')
  assert.ok(gppd)
  assert.strictEqual(gppd.provenance, 'global-measured')
})

test('priority 50 Overture → baseline (OSM-derived inference)', () => {
  const overture = SOURCES_BY_KEY.get('global-overture')
  assert.ok(overture)
  assert.strictEqual(overture.provenance, 'baseline')
})

test('priority 50 Copernicus → baseline (global raster)', () => {
  const copernicus = SOURCES_BY_KEY.get('global-copernicus-glo30')
  assert.ok(copernicus)
  assert.strictEqual(copernicus.provenance, 'baseline')
})

test('priority 10 service-tree → heuristic', () => {
  const st = SOURCES_BY_KEY.get('service-tree-heuristic')
  assert.ok(st)
  assert.strictEqual(st.provenance, 'heuristic')
})

test('priority 10 name-heuristic → heuristic', () => {
  const nh = SOURCES_BY_KEY.get('industrial-name-heuristic')
  assert.ok(nh)
  assert.strictEqual(nh.provenance, 'heuristic')
})

test('PROVENANCE_RANK ordering: measured > heuristic > baseline > none', () => {
  assert.ok(PROVENANCE_RANK['city-measured'] > PROVENANCE_RANK['national-measured'])
  assert.ok(PROVENANCE_RANK['national-measured'] > PROVENANCE_RANK['continental-measured'])
  assert.ok(PROVENANCE_RANK['continental-measured'] > PROVENANCE_RANK['global-measured'])
  assert.ok(PROVENANCE_RANK['global-measured'] > PROVENANCE_RANK['heuristic'])
  assert.ok(PROVENANCE_RANK['heuristic'] > PROVENANCE_RANK['baseline'])
  assert.ok(PROVENANCE_RANK['baseline'] > PROVENANCE_RANK['none'])
})

// ─── shouldOverwrite: provenance rank ──────────────────────────────────────

test('shouldOverwrite: national-measured beats continental-measured', () => {
  // cz-rsd(20, national) vs eu-city(10, continental)
  assert.strictEqual(shouldOverwrite(10, 20), true)
  assert.strictEqual(shouldOverwrite(20, 10), false)
})

test('shouldOverwrite: continental-measured beats heuristic', () => {
  // eu-city(10, continental) vs service-tree(11, heuristic)
  assert.strictEqual(shouldOverwrite(11, 10), true)
  assert.strictEqual(shouldOverwrite(10, 11), false)
})

test('shouldOverwrite: heuristic beats baseline', () => {
  // service-tree(11, heuristic) vs global-overture(9001, baseline)
  assert.strictEqual(shouldOverwrite(9001, 11), true)
  assert.strictEqual(shouldOverwrite(11, 9001), false)
})

test('shouldOverwrite: any tier beats none', () => {
  assert.strictEqual(shouldOverwrite(0, 20), true)  // national
  assert.strictEqual(shouldOverwrite(0, 10), true)  // continental
  assert.strictEqual(shouldOverwrite(0, 11), true)  // heuristic
  assert.strictEqual(shouldOverwrite(0, 9001), true) // baseline
})

// ─── shouldOverwrite: idempotent + unknown ─────────────────────────────────

test('shouldOverwrite: idempotent re-run (existing = self)', () => {
  assert.strictEqual(shouldOverwrite(10, 10), true)
  assert.strictEqual(shouldOverwrite(20, 20), true)
  assert.strictEqual(shouldOverwrite(11, 11), true)
})

test('shouldOverwrite: unknown legacy id treated as overwriteable', () => {
  assert.strictEqual(shouldOverwrite(65535, 10), true)
})

// ─── shouldOverwrite: year tiebreak ────────────────────────────────────────

test('shouldOverwrite: same-rank tiebreak prefers newer year', () => {
  // Synthetic test: two ids with same provenance but different years.
  // Find two national-measured sources with non-null years and different years.
  const nationals = SOURCES.filter(
    (s): s is Source & { year: number } =>
      s.provenance === 'national-measured' && s.year !== null,
  )
  assert.ok(nationals.length >= 2, 'need at least two national-measured sources with years')
  // Look for a pair with different years
  const olderIdx = nationals.findIndex((s) => nationals.some((o) => o.year > s.year))
  if (olderIdx < 0) {
    // All same year — skip with a note
    return
  }
  const older = nationals[olderIdx]
  const newer = nationals.find((s) => s.year > older.year)!
  // Newer year wins regardless of id direction
  if (newer.id > older.id) {
    assert.strictEqual(shouldOverwrite(older.id, newer.id), true)
    assert.strictEqual(shouldOverwrite(newer.id, older.id), false)
  } else {
    // newer has smaller id than older → prove year beats id
    assert.strictEqual(shouldOverwrite(older.id, newer.id), true, 'newer year beats older, even with smaller id')
    assert.strictEqual(shouldOverwrite(newer.id, older.id), false)
  }
})

test('shouldOverwrite: same rank + same year + different id → higher id wins', () => {
  // de-bast-autobahn(22, 2021) vs de-bast-bundesstrassen(23, 2021)
  // both national-measured, same year → higher id wins
  assert.strictEqual(shouldOverwrite(22, 23), true)
  assert.strictEqual(shouldOverwrite(23, 22), false)
})
