//! Minimal deterministic screenshot plan covering seven popup branches and four geographies.

import { createHash } from 'node:crypto'
import { SEMANTIC_LAYERS } from '../../../server/scripts/popup-parity/payload-schema.mjs'
import { canonicalizeRenderedPopupText } from './browser-contract.mjs'

export const POPUP_LAYER_UI = Object.freeze([
  { key: 'road', label: 'Road' },
  { key: 'railway', label: 'Rail' },
  { key: 'aircraft_ground', label: 'Ground' },
  { key: 'aircraft_airborne', label: 'Airborne' },
  { key: 'aircraft_cruise', label: 'Cruise' },
  { key: 'building', label: 'Building' },
  { key: 'industrial', label: 'Industry' },
])

const layerKeys = POPUP_LAYER_UI.map(({ key }) => key).sort()
if (JSON.stringify(layerKeys) !== JSON.stringify([...SEMANTIC_LAYERS].sort())) {
  throw new Error('browser layer controls differ from popup semantic layers')
}

function pointLayerMask(payload) {
  return POPUP_LAYER_UI.reduce(
    (mask, layer, index) =>
      mask | (payload.segments_meta[`${layer.key}_count`] > 0 ? 1 << index : 0),
    0,
  )
}

function lexicographicallyEarlier(a, b) {
  if (!b || a.length !== b.length) return !b || a.length < b.length
  for (let index = 0; index < a.length; index += 1) {
    if (a[index] !== b[index]) return a[index] < b[index]
  }
  return false
}

export function minimumLayerCover(records) {
  const fullMask = (1 << POPUP_LAYER_UI.length) - 1
  const best = Array.from({ length: fullMask + 1 }, () => null)
  best[0] = []
  records.forEach((record, recordIndex) => {
    const recordMask = pointLayerMask(record.payload)
    const snapshot = best.map((selection) => selection && [...selection])
    snapshot.forEach((selection, mask) => {
      if (!selection || recordMask === 0) return
      const combined = mask | recordMask
      const candidate = [...selection, recordIndex]
      if (lexicographicallyEarlier(candidate, best[combined])) best[combined] = candidate
    })
  })
  if (!best[fullMask]) throw new Error('200 browser points do not cover all seven popup layers')
  return best[fullMask].map((index) => records[index])
}

function firstRecord(records, label, predicate) {
  const record = records.find(predicate)
  if (!record) throw new Error(`browser screenshot plan has no ${label} representative`)
  return record
}

function tags(record) {
  return record.point.tags ?? []
}

function openWater(record) {
  const meta = record.payload.segments_meta
  return record.point.group === 'equal-area-v1'
    && Math.abs(record.point.lat) <= 60
    && record.payload.elevation_m === 0
    && ['road', 'railway', 'building', 'industrial']
      .every((layer) => meta[`${layer}_total`] === 0)
}

export function buildRenderPlan(records) {
  const cover = minimumLayerCover(records)
  const geographies = [
    ['city', firstRecord(records, 'city', (record) => tags(record).includes('dense_urban'))],
    ['rural', firstRecord(records, 'rural', (record) => tags(record).includes('rural_open'))],
    ['airport', firstRecord(records, 'airport', (record) => tags(record).includes('hub_approach'))],
    ['sea', firstRecord(records, 'open-water', openWater)],
  ]
  const items = geographies.map(([geography, record]) => ({
    id: `geography-${geography}`,
    point_id: String(record.point.id),
    state: 'sources',
    geography,
  }))
  for (const layer of POPUP_LAYER_UI) {
    const record = cover.find((candidate) =>
      candidate.payload.segments_meta[`${layer.key}_count`] > 0)
    items.push({
      id: `layer-${layer.key}`,
      point_id: String(record.point.id),
      state: 'segments',
      layer: layer.key,
      filter_label: layer.label,
    })
  }
  return {
    schema_version: 1,
    coverage_basis: 'segments_meta.<semantic_layer>_count > 0',
    layer_cover_point_ids: cover.map(({ point }) => String(point.id)),
    items,
  }
}

function groupByPoint(items) {
  const groups = new Map()
  for (const item of items) {
    const group = groups.get(item.point_id) ?? []
    group.push(item)
    groups.set(item.point_id, group)
  }
  return groups
}

async function visibleByRole(page, role, options) {
  const locator = page.getByRole(role, options).filter({ visible: true })
  await locator.waitFor({ state: 'visible' })
  return locator
}

async function collapseLayerCard(page) {
  const button = page.locator('button[title="Hide layers"]:visible')
  if (await button.count()) await button.click()
}

async function isolateLayer(page, item) {
  const tab = await visibleByRole(page, 'button', { name: /^Segments \(\d+\)$/ })
  await tab.click()
  const target = await visibleByRole(page, 'button', { name: item.filter_label, exact: true })
  if (await target.isDisabled()) throw new Error(`${item.id}: layer filter is disabled`)
  await target.click()
  for (const layer of POPUP_LAYER_UI) {
    const button = await visibleByRole(page, 'button', { name: layer.label, exact: true })
    const pressed = await button.getAttribute('aria-pressed')
    const expected = layer.key === item.layer ? 'true' : 'false'
    if (pressed !== expected) {
      throw new Error(`${item.id}: ${layer.key} aria-pressed=${pressed}, expected ${expected}`)
    }
  }
}

async function captureState(browserSession, writer, item) {
  const { page } = browserSession
  if (item.state === 'segments') await isolateLayer(page, item)
  await browserSession.afterPaint()
  const popup = page.locator('[data-testid="detail-popup"]:visible')
  const text = await popup.innerText()
  const png = await page.screenshot({ animations: 'disabled' })
  const screenshot = await writer.writeAttachment(`screenshots/${item.id}.png`, png, 'image/png')
  const renderedText = await writer.writeAttachment(`rendered/${item.id}.txt`, text, 'text/plain; charset=utf-8')
  return {
    ...item,
    screenshot,
    rendered_text: renderedText,
    comparison_text_sha256: createHash('sha256')
      .update(canonicalizeRenderedPopupText(text, item))
      .digest('hex'),
  }
}

export async function captureRenderEvidence(browserSession, records, renderPlan, writer, validateCapture) {
  const recordsById = new Map(records.map((record) => [String(record.point.id), record]))
  const captures = []
  for (const [pointId, items] of groupByPoint(renderPlan.items)) {
    const record = recordsById.get(pointId)
    if (!record) throw new Error(`render plan point is outside browser catalog: ${pointId}`)
    const response = await browserSession.click(record.point, `render-${pointId}`)
    await validateCapture(response, record.point, `render/${pointId}`)
    await collapseLayerCard(browserSession.page)
    const responseSha256 = createHash('sha256').update(response.body).digest('hex')
    for (const item of items.sort((a, b) =>
      Number(a.state === 'segments') - Number(b.state === 'segments'))) {
      captures.push({
        ...await captureState(browserSession, writer, item),
        response_body_sha256: responseSha256,
        requested_coordinates: response.browser.requested_coordinates,
      })
    }
  }
  return { ...renderPlan, captures }
}
