import assert from 'node:assert/strict'
import test from 'node:test'

import {
  BROWSER_ACCEPTANCE_POINT_COUNT,
  loadParityPoints,
} from '../../server/scripts/popup-parity/points.mjs'
import {
  assertCanvasClickCoordinates,
  MAP_COORDINATE_EPSILON_DEGREES,
} from '../scripts/popup-parity/browser-session.mjs'
import {
  assertComparableBrowser,
  canonicalizeRenderedPopupText,
  compareRenderEvidence,
  compareVisibleEvidence,
  pointManifest,
} from '../scripts/popup-parity/browser-contract.mjs'
import {
  buildRenderPlan,
  minimumLayerCover,
  POPUP_LAYER_UI,
} from '../scripts/popup-parity/render-evidence.mjs'

const BROWSER_COORDINATES_SHA256 = '974aabe4aac0a273969666f9c6541c91ddf594b415918aba84771717f8968cda'

function meta(enabled = []) {
  const value = { total_count: enabled.length, truncated: false }
  for (const { key } of POPUP_LAYER_UI) {
    value[`${key}_count`] = enabled.includes(key) ? 1 : 0
    value[`${key}_total`] = enabled.includes(key) ? 1 : 0
  }
  return value
}

function record(id, tags, group, enabled, elevation_m = 1, lat = 50) {
  return {
    point: { id, lat, lng: 14, tags, group },
    payload: { elevation_m, segments_meta: meta(enabled) },
  }
}

test('browser catalog pins 200 distinct locations, measured group counts, and all E180 probes', async () => {
  const pointSet = await loadParityPoints()
  assert.equal(pointSet.browserPoints.length, BROWSER_ACCEPTANCE_POINT_COUNT)
  assert.equal(new Set(pointSet.browserPoints.map(({ lat, lng }) =>
    JSON.stringify([lat, lng === 180 ? -180 : lng]))).size, BROWSER_ACCEPTANCE_POINT_COUNT)
  assert.equal(pointSet.browserCoordinatesSha256, BROWSER_COORDINATES_SHA256)
  assert.equal(
    pointSet.browserPoints.filter(({ group }) => group === 'curated-world').length,
    61,
  )
  assert.equal(
    pointSet.browserPoints.filter(({ group }) => group === 'equal-area-v1').length,
    139,
  )
  const manifest = pointManifest(pointSet)
  assert.equal(manifest.curated_count, 61)
  assert.equal(manifest.equal_area_count, 139)
  assert.equal(manifest.total_count, 200)
  assert.deepEqual(
    pointSet.browserPoints.filter(({ role }) => role?.includes('180')).map(({ role }) => role),
    ['east_180_boundary', 'east_180_inside', 'west_180_inside', 'west_180_boundary'],
  )
})

test('canvas click coordinates allow only the established projection epsilon and wrap E180', () => {
  assert.deepEqual(
    assertCanvasClickCoordinates({ lat: 50 + 1e-12, lng: -180 }, { lat: 50, lng: 180 }, 'seam'),
    { lat_delta: 1.0018652574217413e-12, wrapped_lng_delta: 0 },
  )
  assert.throws(
    () => assertCanvasClickCoordinates(
      { lat: 50 + MAP_COORDINATE_EPSILON_DEGREES * 2, lng: 14 },
      { lat: 50, lng: 14 },
      'drift',
    ),
    /canvas click requested/,
  )
})

test('render plan uses count-based minimum cover and city, rural, airport, sea views', () => {
  const layers = POPUP_LAYER_UI.map(({ key }) => key)
  const records = [
    record('all-layers', ['suburban'], 'curated-world', layers),
    record('city', ['dense_urban'], 'curated-world', ['road', 'building']),
    record('rural', ['rural_open'], 'curated-world', ['road']),
    record('airport', ['hub_approach'], 'curated-world', ['aircraft_airborne']),
    record('sea', [], 'equal-area-v1', ['aircraft_cruise'], 0, 40),
  ]
  assert.deepEqual(minimumLayerCover(records).map(({ point }) => point.id), ['all-layers'])
  const plan = buildRenderPlan(records)
  assert.deepEqual(plan.layer_cover_point_ids, ['all-layers'])
  assert.equal(plan.items.length, 11)
  assert.deepEqual(
    plan.items.filter(({ state }) => state === 'segments').map(({ layer }) => layer),
    layers,
  )
  assert.deepEqual(
    plan.items.filter(({ state }) => state === 'sources').map(({ geography }) => geography),
    ['city', 'rural', 'airport', 'sea'],
  )
})

test('rendered cruise rows retain values but never compare cell identifiers literally', () => {
  const item = { layer: 'aircraft_cruise' }
  assert.equal(
    canonicalizeRenderedPopupText('Cruise over 871e354c3\n8.0 km\n19.3 dB', item),
    canonicalizeRenderedPopupText('Cruise over z9/276/172\n8.0 km\n19.3 dB', item),
  )
  assert.notEqual(
    canonicalizeRenderedPopupText('Cruise over 871e354c3\n8.0 km\n19.3 dB', item),
    canonicalizeRenderedPopupText('Cruise over z9/276/172\n8.0 km\n18.3 dB', item),
  )
})

test('cruise labels never exempt the rest of a screenshot from comparison', async () => {
  const capture = {
    id: 'layer-aircraft_cruise', point_id: 'sea', state: 'segments',
    layer: 'aircraft_cruise', comparison_text_sha256: 'same-text',
    screenshot: { sha256: 'reference-png' },
  }
  for (const candidateHash of ['reference-png', 'different-png']) {
    const [result] = await compareRenderEvidence(
      { captures: [capture] },
      { captures: [{ ...capture, screenshot: { sha256: candidateHash } }] },
      () => { throw new Error('unexpected attachment read') },
    )
    assert.equal(result.screenshot_bytes_identical, candidateHash === 'reference-png')
    assert.equal(result.rendered_text_exact, true)
  }
})

test('browser acceptance rejects mocked references and mismatched rendering environments', () => {
  const settings = {
    network_mocking: false, engine: 'chromium', version: 'same-version',
    viewport: { width: 1280, height: 720 }, basemap: 'standard',
    executable_path: '/one/browser',
  }
  assert.doesNotThrow(() => assertComparableBrowser(settings, {
    ...settings, executable_path: '/another/browser',
  }))
  assert.throws(() => assertComparableBrowser({ basemap: 'mocked' }, settings), /unmodified live/)
  assert.throws(() => assertComparableBrowser(settings, { ...settings, version: 'different' }), /settings differ/)
})

test('every point reports screenshot and text differences independently', () => {
  const reference = { browser: {
    rendered_text: '50.0 dB', rendered_text_sha256: 'text-hash', screenshot: { sha256: 'image-hash' },
  } }
  const state = { text_equal: 0, screenshot_equal: 0, differences: [] }
  compareVisibleEvidence(reference, reference, { id: 'identical' }, state)
  compareVisibleEvidence(reference, { browser: {
    ...reference.browser, screenshot: { sha256: 'different-image' },
  } }, { id: 'image-change' }, state)
  compareVisibleEvidence(reference, { browser: {
    ...reference.browser, rendered_text: '40.0 dB', rendered_text_sha256: 'different-text',
  } }, { id: 'text-change' }, state)
  assert.equal(state.text_equal, 2)
  assert.equal(state.screenshot_equal, 2)
  assert.deepEqual(state.differences.map((item) => [
    item.point_id, item.rendered_text_exact, item.screenshot_bytes_identical,
  ]), [['image-change', true, false], ['text-change', false, true]])
})
