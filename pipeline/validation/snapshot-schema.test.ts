import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { boundedCoveragePercent, REPO_ROOT, validateSnapshot, type Snapshot } from './lib.ts'
import { loadApprovedSnapshots, validateApprovedSnapshotIdentity } from './snapshot-loader.mjs'

const expected = {
  'barcelona-xarxa-soroll.2025.json': ['barcelona-xarxa-soroll', 'ES', 154, 'upper_bound', 2],
  'dublin-sonitus.2025.json': ['dublin-sonitus', 'IE', 14, 'upper_bound', 2],
  'eba-laermmonitoring.2023.json': ['eba-laermmonitoring', 'DE', 19, 'trend_only', null],
  'zrh-nmt.2024.json': ['zrh-nmt', 'CH', 4, 'trend_only', null],
  'lkpr-tanos.2026.json': ['lkpr-tanos', 'CZ', 14, 'trend_only', null],
} as const

test('adapter coverage rounding cannot publish more than 100 percent', () => {
  assert.equal(boundedCoveragePercent(0.7), 70)
  assert.equal(boundedCoveragePercent(1), 100)
  assert.equal(boundedCoveragePercent(1.005), 100)
  assert.throws(() => boundedCoveragePercent(1.006), /0\.\.1\.005/)
})

test('approved measurement catalog is internally consistent', () => {
  const snapshots = new Map<string, Snapshot>()
  for (const { file, path, snapshot } of loadApprovedSnapshots(REPO_ROOT)) {
    validateSnapshot(snapshot, path)
    snapshots.set(file, snapshot)
  }
  assert.deepEqual([...snapshots.keys()], Object.keys(expected), 'only the explicit allow-list is discoverable')
  for (const [file, [network, country, count, comparison, tolerance]] of Object.entries(expected)) {
    const snapshot = snapshots.get(file)!
    assert.equal(snapshot.schema_version, 2)
    assert.equal(snapshot.network, network)
    assert.equal(snapshot.country_code, country)
    assert.equal(snapshot.stations.length, count)
    assert.equal(snapshot.comparison_mode, comparison)
    assert.equal(snapshot.comparison_tolerance_db, tolerance)
    assert.ok(snapshot.commensurability.receiver_convention)
    assert.ok(snapshot.stations.every(station => Number.isFinite(station[snapshot.measured_metric_field])))
  }

  // Reviewed source fixes that must not silently regress during refreshes.
  const dublinIds = new Set(snapshots.get('dublin-sonitus.2025.json')!.stations.map(station => station.station_id))
  assert.ok(dublinIds.has('10.1.1.1') && dublinIds.has('10.1.1.7'))
  assert.ok(!dublinIds.has('01528') && !dublinIds.has('01534'), 'replacement instruments must not double-weight a site')

  const barcelona = snapshots.get('barcelona-xarxa-soroll.2025.json')!
  assert.deepEqual(barcelona.tags, ['dense_urban'])
  assert.equal(barcelona.stations.filter(station => station.tags?.includes('pedestrian_zone')).length, 10)

  const eba = snapshots.get('eba-laermmonitoring.2023.json')!
  assert.deepEqual(eba.tags, ['near', 'rail_count_measured'])
  const registry = JSON.parse(readFileSync(resolve(REPO_ROOT, 'pipeline/validation/eba-stations.json'), 'utf8')) as {
    stations: Record<string, { lat: number; lng: number }>
  }
  assert.equal(Object.keys(registry.stations).length, eba.stations.length)
  for (const station of eba.stations) {
    assert.deepEqual(
      [station.lat, station.lng],
      [registry.stations[station.station_id]?.lat, registry.stations[station.station_id]?.lng],
      `${station.station_id} receiver coordinates`,
    )
  }
})

test('snapshot validator rejects data that changes comparison meaning', () => {
  const source = loadApprovedSnapshots<Snapshot>(REPO_ROOT)
    .find(entry => entry.file === 'barcelona-xarxa-soroll.2025.json')!.snapshot

  const missingMode = structuredClone(source) as unknown as Record<string, unknown>
  delete missingMode.comparison_mode
  assert.throws(() => validateSnapshot(missingMode), /unknown comparison_mode/)

  const twoSidedAmbient = structuredClone(source)
  twoSidedAmbient.comparison_mode = 'two_sided'
  assert.throws(() => validateSnapshot(twoSidedAmbient), /total_ambient cannot use a two_sided comparison/)

  const badTag = structuredClone(source)
  badTag.stations[0].tags = ['not_in_the_factor_catalog']
  assert.throws(() => validateSnapshot(badTag), /unknown factor tag/)

  const badTimestamp = structuredClone(source)
  badTimestamp.fetched_at = '2026-07-12'
  assert.throws(() => validateSnapshot(badTimestamp), /canonical UTC ISO instant/)

  const missingMeasured = structuredClone(source)
  delete missingMeasured.stations[0].lden
  assert.throws(() => validateSnapshot(missingMeasured), /measured metric lden must be finite/)

  const inconsistentLden = structuredClone(source)
  inconsistentLden.stations[0].lden = 1
  assert.throws(() => validateSnapshot(inconsistentLden), /inconsistent with the period_split/)

  const shortCoverage = structuredClone(source)
  shortCoverage.stations[0].months_covered = 8
  assert.throws(() => validateSnapshot(shortCoverage), /months_covered in 9\.\.12/)

  const partialOverride = structuredClone(source)
  partialOverride.stations[0].commensurability = { coord_uncertainty_m: 20 }
  assert.doesNotThrow(() => validateSnapshot(partialOverride), 'partial override must retain network defaults')

  const nonComparableOverride = structuredClone(source)
  nonComparableOverride.stations[0].commensurability = { metric_variant: 'laeq_windows' }
  assert.throws(() => validateSnapshot(nonComparableOverride), /effective band-capable metric_variant/)

  assert.throws(
    () => validateApprovedSnapshotIdentity(source, 'other-network.2025.json'),
    /filename and snapshot network\/year disagree/,
  )
})

const payloadFields: Record<string, string[]> = {
  'barcelona-xarxa-soroll.2025.json': ['station_id', 'name', 'lat', 'lng', 'ld', 'le', 'ln', 'lden', 'months_covered', 'coverage_pct'],
  'dublin-sonitus.2025.json': ['station_id', 'name', 'lat', 'lng', 'ld', 'le', 'ln', 'lden', 'months_covered', 'coverage_pct'],
  'eba-laermmonitoring.2023.json': ['station_id', 'name', 'lat', 'lng', 'laeq_24h', 'laeq_tag_0622', 'laeq_nacht_2206', 'trains_per_day', 'freight_trains_per_day', 'trains_night', 'mean_speed_kmh', 'mean_train_length_m'],
  'zrh-nmt.2024.json': ['station_id', 'name', 'lat', 'lng', 'laeq_tag_0622'],
  'lkpr-tanos.2026.json': ['station_id', 'name', 'lat', 'lng', 'month', 'laeq_aircraft_day_0622', 'laeq_aircraft_night_2206'],
}
const payloadSha256: Record<string, string> = {
  'barcelona-xarxa-soroll.2025.json': '56228b9ca7fae304697395c4ace799763a26b151e9bd82afd3bcc3c07786f513',
  'dublin-sonitus.2025.json': 'cd72ec4c81a5f67a8b43e7e758f3b21af3624004e35519af0102dfebfdf12451',
  'eba-laermmonitoring.2023.json': 'e9945f0e5f2acc18706a00483edb44f62ee6e1b8f51f1730314486c27ca1f8d2',
  'zrh-nmt.2024.json': '8593f8304902247e3a96021a1e551c8cfce86ba4e050a9ae38aa0bc36e82fbdc',
  'lkpr-tanos.2026.json': '6f284ec4b3ceebc63d41e4b64508832afa0e32e3d34e435afd951f47f5b7f115',
}

test('reviewed station measurements and coordinates remain pinned', () => {
  for (const { file, snapshot } of loadApprovedSnapshots<Snapshot>(REPO_ROOT)) {
    const payload = snapshot.stations.map(station =>
      Object.fromEntries(payloadFields[file].map(field => [field, station[field]])))
    assert.equal(createHash('sha256').update(JSON.stringify(payload)).digest('hex'), payloadSha256[file], file)
  }
})
