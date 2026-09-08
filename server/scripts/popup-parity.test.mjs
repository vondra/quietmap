//! Hermetic contract tests for worldwide popup reference capture and comparison.

import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { createCaptureWriter, openReferenceArtifact } from './popup-parity/artifact.mjs'
import {
  canonicalizePopup,
  compareCanonical,
  comparisonReport,
} from './popup-parity/contract.mjs'
import { openEndpointSession } from './popup-parity/endpoint.mjs'
import {
  generateEqualAreaPoints,
  loadParityPoints,
} from './popup-parity/points.mjs'
import { validatePopupPayload } from './popup-parity/payload-schema.mjs'

const HASHES = {
  world_coordinates_sha256: 'e3860bf57725c4e002846e33d524aaef5fbce842e89c1aec66f9a37f0c97882d',
  equal_area_coordinates_sha256: 'ed69aadea0767ec260a35c900a86d0f2838ce2dbf85d0f2daf66c31d2ca7195c',
  all_coordinates_sha256: '9ba6897d3d723edbf7be19b749df9dca0b49d4a6eee949ce4bef093c187909c2',
}
const POINT = { id: 'fixture', lat: 10, lng: 20 }
const VARIANTS = {
  full: 0,
  free_field: 0,
  no_terrain: 0,
  no_screening: 0,
  no_vegetation: 0,
  no_ground: 0,
  no_atmospheric: 0,
}

function cnossos() {
  return {
    model: 'cnossos',
    baseline: {},
    path_profile: {
      t: [], elevation_m: [], forest_u8: [], imd_u8: [],
      dist_m: 0, step_m_med: 0, src_lat: 10, src_lon: 20,
      rcv_lat: 10, rcv_lon: 20, src_alt_m: 0, rcv_alt_m: 0,
    },
    terrain: {},
    screening: {},
    vegetation: {},
    ground: {},
    lw_bands: {},
    lw_db_a: {},
    received_bands: {},
  }
}

function doc29() {
  return {
    model: 'doc29',
    sel_npd_db: 0,
    delta_v_db: 0,
    delta_i_db: 0,
    lambda_db: 0,
    delta_f_db: 0,
    d_p_m: 1,
    lateral_m: 0,
    beta_deg: 90,
    seg_len_m: 1,
    d_bar_m: 1,
    installation: 'wing',
    cffk_fast_path: true,
    screening_kind: 'none',
    screening_db: 0,
  }
}

function emission(layer, currentCruise = false) {
  const values = {
    road: {
      kind: 'road', aadt_light: 1, aadt_medium: 0, aadt_heavy: 0, aadt_moto: 0,
      speed_kmh: 50, surface_corr_db: 0, surface: 'asphalt',
      traffic_source: 'default_by_class', source_id: 0, road_class: 'residential',
      bridge: false, tunnel: false, oneway: false, lanes: 2,
    },
    railway: {
      kind: 'railway', trains_passenger: 1, trains_freight: 0,
      trains_passenger_source: 'arrow', trains_freight_source: 'default_by_type',
      source_id: 0, speed_kmh: 80, bridge: false, highspeed: false,
      rail_type: 'rail', service: false,
    },
    aircraft_ground: {
      kind: 'aircraft_ground', class: 'runway', observed_movements: 1,
      modeled_movements: 1, arrivals_per_day: 1, departures_per_day: 1,
      gse_per_day: [0, 0, 0], class_mix: [], osm_ref: null,
    },
    aircraft_airborne: {
      kind: 'aircraft_airborne', class: 'JET_MEDIUM', callsign: 'QM1',
      aircraft_type: 'A320', cpa_distance_m: 100, altitude_m_at_cpa: 1000,
      is_departure: true, icao_hex: 'abcdef', start_unix: 1,
    },
    aircraft_cruise: {
      kind: 'aircraft_cruise', n_unique_flights: 2, rep_alt_m: 10_000,
      [currentCruise ? 'square' : 'r7_hex']: currentCruise ? 'z9/1/2' : '871111111ffffff',
    },
    building: { kind: 'building', building_type: 'house', height_m: 6, floors: 2, area_m2: 80 },
    industrial: {
      kind: 'industrial', source_type: 'industrial_area', area_m2: 100,
      effective_area_source_dist_m: 20,
    },
  }
  return values[layer]
}

function segment(layer, currentCruise = false) {
  const aircraftSubtype = {
    aircraft_ground: 1,
    aircraft_airborne: 2,
    aircraft_cruise: 3,
  }[layer]
  const value = {
    kind: aircraftSubtype ? 'aircraft' : layer,
    osm_id: aircraftSubtype ? null : 1,
    segment_idx: 0,
    name: layer === 'aircraft_cruise' ? (currentCruise ? 'Cruise over z9/1/2' : 'Cruise over 871111111') : layer,
    subtype: layer,
    is_dominant_of_group: false,
    start_lat: 10,
    start_lon: currentCruise ? 21 : 20,
    end_lat: 10,
    end_lon: currentCruise ? 21 : 20,
    cp_lat: 10,
    cp_lon: currentCruise ? 21 : 20,
    length_m: 1,
    dist_m: 1,
    d_slant_m: 1,
    bridge: false,
    tunnel: false,
    emission: emission(layer, currentCruise),
    propagation: ['aircraft_airborne', 'aircraft_cruise'].includes(layer) ? doc29() : cnossos(),
    received_lden: { ...VARIANTS },
  }
  if (aircraftSubtype) value.aircraft_subtype = aircraftSubtype
  if (layer === 'aircraft_cruise') {
    value[currentCruise ? 'cell_polygon' : 'hex_polygon'] = [[9, 19], [9, 21], [11, 21], [9, 19]]
    value.cruise_buckets = []
    value.cruise_top_flights = []
  }
  return value
}

function payload(current = false) {
  const layers = [
    'road', 'railway', 'aircraft_ground', 'aircraft_airborne',
    'aircraft_cruise', 'building', 'industrial',
  ]
  const meta = { total_count: 7, truncated: false }
  for (const layer of layers) {
    meta[`${layer}_count`] = 1
    meta[`${layer}_total`] = 1
  }
  return {
    [current ? 'center' : 'h3_center']: [10, 20],
    elevation_m: 1,
    total_lden: null,
    total_lden_free: null,
    sources: ['road', 'railway', 'building', 'industrial', 'aircraft'].map((source_type) => ({
      source_type, lden: null, lden_free: null, ld: null, le: null, ln: null,
      segment_count: 1, displayed_count: 0,
    })),
    top_contributors: [],
    other_sources_lden: null,
    compute_time_ms: 1,
    segments: layers.map((layer) => segment(layer, current)),
    segments_meta: meta,
    timings: {
      load_ms: 0, collect_ms: 0, road_ms: 0, rail_ms: 0,
      building_ms: 0, industrial_ms: 0, aircraft_airborne_ms: 0,
      aircraft_cruise_ms: 0, aircraft_ground_ms: 0,
    },
  }
}

test('64 curated plus 200 deterministic equal-area points have pinned coordinates and boundaries', async () => {
  const first = await loadParityPoints()
  const second = await loadParityPoints()
  assert.equal(first.points.length, 264)
  assert.deepEqual(first.hashes, HASHES)
  assert.deepEqual(first, second)
  const generated = generateEqualAreaPoints()
  assert.equal(generated.length, 200)
  assert.equal(generated[0].lat, -generated.at(-1).lat)
  assert.ok(generated[0].lat < 85.051129)
  assert.equal(generated[0].lng, 180)
  assert.equal(generated[1].lng, 179.999999)
  assert.equal(generated.at(-1).lng, -180)
  assert.equal(generated.at(-2).lng, -179.999999)
  assert.equal(new Set(first.points.map(({ id }) => String(id))).size, 264)
})

test('strict schema accepts all seven layers and canonicalizes only known dev1 aliases', () => {
  const legacy = payload(false)
  const current = payload(true)
  validatePopupPayload(legacy, POINT, 'legacy')
  validatePopupPayload(current, POINT, 'current')
  const state = compareCanonical(canonicalizePopup(legacy), canonicalizePopup(current), POINT.id)
  const report = comparisonReport(state)
  assert.deepEqual(report.structural_differences, [])
  assert.deepEqual(report.categorical_differences, [])
  assert.equal(report.numeric['points[\"fixture\"].elevation_m'].absolute_delta_max, 0)
  assert.ok(!JSON.stringify(canonicalizePopup(legacy)).includes('871111111'))
  assert.ok(!JSON.stringify(canonicalizePopup(current)).includes('z9/1/2'))
})

test('schema fails unknown fields, invalid aircraft subtype, meta drift, and energy drift', () => {
  const unknown = payload()
  unknown.guessed = true
  assert.throws(() => validatePopupPayload(unknown, POINT), /unknown key "guessed"/)
  const subtype = payload()
  subtype.segments[2].aircraft_subtype = 4
  assert.throws(() => validatePopupPayload(subtype, POINT), /subtype must be 1, 2, or 3/)
  const meta = payload()
  meta.segments_meta.road_count = 0
  assert.throws(() => validatePopupPayload(meta, POINT), /road_count does not match/)
  const energy = payload()
  energy.sources[0].lden = 50
  energy.sources[0].lden_free = 50
  energy.total_lden = 51
  energy.total_lden_free = 50
  assert.throws(() => validatePopupPayload(energy, POINT), /linear-energy sum/)
})

test('numeric differences are reported without an invented acoustic verdict', () => {
  const reference = canonicalizePopup(payload())
  const candidatePayload = payload(true)
  candidatePayload.elevation_m = 2.25
  const report = comparisonReport(compareCanonical(reference, canonicalizePopup(candidatePayload), POINT.id))
  const elevation = report.numeric['points[\"fixture\"].elevation_m']
  assert.equal(elevation.signed_delta_mean, 1.25)
  assert.equal(elevation.absolute_delta_max, 1.25)
  assert.equal(report.policy.acoustic_tolerances, null)
  assert.equal(report.policy.numeric_verdict, 'diagnostic_only')
})

test('synthetic airport identity normalization is disclosed and retains airport quantities', () => {
  const legacy = payload()
  legacy.top_contributors = [{ source_type: 'aircraft', name: 'Airport', metadata: {
    kind: 'aircraft', airport_key: 'auto-legacy', observed_movements_per_day: 10,
  } }]
  const candidate = structuredClone(legacy)
  candidate.top_contributors[0].metadata.airport_key = 'strip:new'
  candidate.top_contributors[0].metadata.observed_movements_per_day = 12
  const report = comparisonReport(compareCanonical(canonicalizePopup(legacy), canonicalizePopup(candidate), POINT.id))
  assert.deepEqual(report.categorical_differences, [])
  assert.equal(report.numeric['points["fixture"].top_contributors[].metadata.observed_movements_per_day'].absolute_delta_max, 2)
  assert.equal(report.policy.synthetic_airport_identity, 'auto- and strip: airport_key identifiers are normalized; names and quantities remain compared')
})

test('reference artifact writes out-of-order results deterministically and verifies both hashes', async (t) => {
  const root = await mkdtemp(join(tmpdir(), 'popup-parity-test-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  const destination = join(root, 'reference')
  const writer = await createCaptureWriter(destination, 2)
  const entry = (index) => ({
    schema_version: 1,
    index,
    point: { id: String(index), lat: index, lng: index },
    instance: 'fixture',
    http_status: 200,
    content_type: 'application/json',
    elapsed_ms: 1,
    body_sha256: '0'.repeat(64),
    payload: { index },
  })
  await writer.append(1, entry(1))
  await writer.append(0, entry(0))
  const attachment = await writer.writeAttachment('screenshots/road.png', Buffer.from('png'), 'image/png')
  const { manifest } = await writer.finish({
    created_at: '2026-09-04T00:00:00.000Z',
    source: {},
    points: { hashes: HASHES },
    request_policy: {},
    coverage: {},
  })
  assert.equal(manifest.artifact.entry_count, 2)
  assert.equal(manifest.attachments[0].sha256, attachment.sha256)
  const opened = await openReferenceArtifact(destination, HASHES)
  assert.equal((await opened.readAttachment('screenshots/road.png')).toString(), 'png')
  const seen = []
  for await (const value of opened.entries) seen.push(value.index)
  assert.deepEqual(seen, [0, 1])
  await assert.rejects(createCaptureWriter(destination, 1), /already exists/)
})

test('endpoint session derives routes, pins instance/cohort, and sends no full query', async () => {
  const cohort = {
    schema_version: 1,
    cohort_id: '1'.repeat(64),
    cache_ttl_ms: 0,
    data_year: '2024',
    runtime_sha256: '2'.repeat(64),
    prepared_sha256: '3'.repeat(64),
  }
  const urls = []
  const fetchImpl = async (url) => {
    urls.push(String(url))
    const body = new URL(url).pathname === '/api/validation/cohort'
      ? JSON.stringify(cohort)
      : new URL(url).pathname === '/api/noise-onfly-v2'
        ? JSON.stringify({ ok: true })
        : JSON.stringify({ status: 'ok' })
    return new Response(body, {
      status: 200,
      headers: { 'x-0db-instance': 'fixture-instance', 'content-type': 'application/json' },
    })
  }
  const session = await openEndpointSession('https://example.test', {
    expectedInstance: 'fixture-instance',
    fetchImpl,
  })
  const result = await session.query({ id: 'x', lat: 1, lng: 2 })
  assert.deepEqual(result.payload, { ok: true })
  await session.finish()
  assert.match(urls[2], /noise-onfly-v2\?lat=1&lng=2$/)
  assert.ok(urls.every((url) => !url.includes('full=')))
  assert.equal(urls.filter((url) => url.endsWith('/api/health')).length, 2)
  assert.equal(urls.filter((url) => url.endsWith('/api/validation/cohort')).length, 2)
})
