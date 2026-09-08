//! Browser capture schema, artifact metadata, and exact non-acoustic comparisons.

import { createHash } from 'node:crypto'
import { lookup } from 'node:dns/promises'
import { isDeepStrictEqual } from 'node:util'
import { REQUEST_INTERVAL_MS } from '../../../server/scripts/popup-parity/endpoint.mjs'
import { validatePopupPayload } from '../../../server/scripts/popup-parity/payload-schema.mjs'
import { assertCanvasClickCoordinates } from './browser-session.mjs'

function sha256(value) {
  return createHash('sha256').update(value).digest('hex')
}

function pointRecord(point) {
  return Object.fromEntries(
    ['id', 'lat', 'lng', 'group', 'role', 'mode', 'host', 'anchor_type', 'tags']
      .filter((key) => Object.hasOwn(point, key))
      .map((key) => [key, point[key]]),
  )
}

export function validateBrowserCapture(capture, point, label) {
  if (capture.http_status !== 200 || !capture.content_type.toLowerCase().startsWith('application/json')) {
    throw new Error(`${label}: expected HTTP 200 application/json`)
  }
  const coordinates = capture.browser?.requested_coordinates
  if (!Array.isArray(coordinates) || coordinates.length !== 2) {
    throw new Error(`${label}: missing browser request coordinates`)
  }
  const requested = { lat: coordinates[0], lng: coordinates[1] }
  assertCanvasClickCoordinates(requested, point, label)
  validatePopupPayload(capture.payload, requested, label)
  if (sha256(capture.browser.rendered_text) !== capture.browser.rendered_text_sha256) {
    throw new Error(`${label}: rendered text SHA-256 mismatch`)
  }
  return capture
}

export function entryFromCapture(capture, point, index) {
  return {
    schema_version: 1,
    index,
    point: pointRecord(point),
    instance: capture.instance,
    http_status: capture.http_status,
    content_type: capture.content_type,
    elapsed_ms: capture.elapsed_ms,
    body_sha256: sha256(capture.body),
    payload: capture.payload,
    browser: capture.browser,
  }
}

export function validateReferenceEntry(entry, point) {
  for (const key of ['id', 'lat', 'lng']) {
    if (String(entry.point[key]) !== String(point[key])) {
      throw new Error(`reference entry ${entry.index}: point ${key} differs from browser catalog`)
    }
  }
  if (entry.instance === '' || entry.browser === undefined) {
    throw new Error(`reference entry ${entry.index}: not a browser capture`)
  }
  validateBrowserCapture(
    { ...entry, body: Buffer.from(JSON.stringify(entry.payload)) },
    point,
    `reference/${point.id}`,
  )
  return entry
}

export function pointManifest(pointSet) {
  return {
    curated_count: pointSet.browserPoints.filter(({ group }) => group === 'curated-world').length,
    equal_area_count: pointSet.browserPoints.filter(({ group }) => group === 'equal-area-v1').length,
    total_count: pointSet.browserPoints.length,
    diagnostic_catalog_count: pointSet.points.length,
    hashes: {
      ...pointSet.hashes,
      browser_coordinates_sha256: pointSet.browserCoordinatesSha256,
    },
  }
}

export function requestPolicy() {
  return {
    mode: 'default',
    full: false,
    mechanism: 'maplibre_canvas_center_click',
    starts_per_second: 1000 / REQUEST_INTERVAL_MS,
    concurrency: 1,
    acoustic_tolerances: null,
    screenshot_tolerances: null,
  }
}

export async function sourceManifest(endpoint, rejectedReferences, lookupImpl = lookup) {
  const hostname = new URL(endpoint.popupUrl).hostname
  const addresses = await lookupImpl(hostname, { all: true })
  return {
    url: endpoint.popupUrl,
    page_origin: new URL(endpoint.popupUrl).origin,
    hostname,
    resolved_addresses: [...new Set(addresses.map(({ address }) => address))].sort(),
    instance: endpoint.instance,
    cohort: endpoint.cohort,
    rejected_references: rejectedReferences,
  }
}

export function compareVisibleEvidence(reference, candidate, point, state) {
  const textEqual = reference.browser.rendered_text === candidate.browser.rendered_text
  const screenshotEqual = reference.browser.screenshot.sha256 === candidate.browser.screenshot.sha256
  if (textEqual) state.text_equal += 1
  if (screenshotEqual) state.screenshot_equal += 1
  if (!textEqual || !screenshotEqual) {
    state.differences.push({
      point_id: String(point.id),
      rendered_text_exact: textEqual,
      screenshot_bytes_identical: screenshotEqual,
      reference_sha256: reference.browser.rendered_text_sha256,
      candidate_sha256: candidate.browser.rendered_text_sha256,
      reference_screenshot_sha256: reference.browser.screenshot.sha256,
      candidate_screenshot_sha256: candidate.browser.screenshot.sha256,
    })
  }
}

export function assertComparableBrowser(reference, candidate) {
  if (reference?.network_mocking !== false || candidate?.network_mocking !== false) {
    throw new Error('browser comparison requires unmodified live websites; recapture the reference')
  }
  const settings = ({ executable_path: _executablePath, ...value }) => value
  if (!isDeepStrictEqual(settings(reference), settings(candidate))) {
    throw new Error('reference and candidate browser settings differ')
  }
}

export function canonicalizeRenderedPopupText(text, item) {
  return item.layer === 'aircraft_cruise'
    ? text.replace(/^Cruise over .+$/gm, 'Cruise cell')
    : text
}

export async function compareRenderEvidence(referencePlan, candidatePlan, readReferenceAttachment) {
  const candidate = new Map(candidatePlan.captures.map((capture) => [capture.id, capture]))
  return Promise.all(referencePlan.captures.map(async (reference) => {
    const current = candidate.get(reference.id)
    if (!current || current.point_id !== reference.point_id || current.state !== reference.state) {
      throw new Error(`candidate render evidence differs from reference plan at ${reference.id}`)
    }
    let referenceTextSha256 = reference.comparison_text_sha256
    if (!referenceTextSha256) {
      const text = await readReferenceAttachment(reference.rendered_text.file)
      referenceTextSha256 = sha256(canonicalizeRenderedPopupText(text.toString('utf8'), reference))
    }
    const representationSpecificCruiseLabel = reference.layer === 'aircraft_cruise'
    return {
      id: reference.id,
      point_id: reference.point_id,
      state: reference.state,
      screenshot_bytes_identical: reference.screenshot.sha256 === current.screenshot.sha256,
      rendered_text_exact: referenceTextSha256 === current.comparison_text_sha256,
      representation_note: representationSpecificCruiseLabel
        ? 'Cruise cell labels may differ; every screenshot difference still requires inspection'
        : null,
      reference_screenshot_sha256: reference.screenshot.sha256,
      candidate_screenshot_sha256: current.screenshot.sha256,
    }
  }))
}
