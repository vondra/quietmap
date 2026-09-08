//! Cohort-pinned orchestration for browser popup reference and candidate artifacts.

import {
  createCaptureWriter,
  openReferenceArtifact,
} from '../../../server/scripts/popup-parity/artifact.mjs'
import {
  canonicalizePopup,
  compareCanonical,
  comparisonReport,
  createComparisonAccumulator,
  performanceSnapshot,
} from '../../../server/scripts/popup-parity/contract.mjs'
import {
  inspectEndpoint,
  openEndpointSession,
} from '../../../server/scripts/popup-parity/endpoint.mjs'
import { loadParityPoints } from '../../../server/scripts/popup-parity/points.mjs'
import {
  assertCompleteSuiteCoverage,
  payloadCoverage,
} from '../../../server/scripts/popup-parity/payload-schema.mjs'
import {
  assertComparableBrowser,
  compareRenderEvidence,
  compareVisibleEvidence,
  entryFromCapture,
  pointManifest,
  requestPolicy,
  sourceManifest,
  validateBrowserCapture,
  validateReferenceEntry,
} from './browser-contract.mjs'
import { openMapBrowserSession } from './browser-session.mjs'
import {
  buildRenderPlan,
  captureRenderEvidence,
} from './render-evidence.mjs'

function progress(done, total, label) {
  if (done === total || done % 10 === 0) process.stderr.write(`${label}: ${done}/${total}\n`)
}

async function capturePoints(browser, writer, points, referenceEntries, compare) {
  const coverages = []
  const records = []
  for (const [index, point] of points.entries()) {
    const capture = validateBrowserCapture(
      await browser.click(point),
      point,
      `browser/${point.id}`,
    )
    await browser.afterPaint()
    capture.browser.screenshot = await writer.writeAttachment(
      `screenshots/points/${String(index).padStart(3, '0')}.png`,
      await browser.page.screenshot({ animations: 'disabled' }),
      'image/png',
    )
    const entry = entryFromCapture(capture, point, index)
    await writer.append(index, entry)
    coverages.push(payloadCoverage(capture.payload))
    records.push({ index, point, payload: capture.payload })
    compare?.(entry, referenceEntries?.[index], point)
    progress(index + 1, points.length, referenceEntries ? 'compare-browser' : 'capture-browser')
  }
  return { coverages, records }
}

async function openRuntime(options, pointCount, dependencies) {
  const writer = await (dependencies.createWriter ?? createCaptureWriter)(options.output, pointCount)
  let browser
  try {
    const endpoint = await (dependencies.openSession ?? openEndpointSession)(options.url, {
      expectedInstance: options.instance,
      fetchImpl: dependencies.fetchImpl,
    })
    browser = await (dependencies.openBrowser ?? openMapBrowserSession)(
      new URL(endpoint.popupUrl).origin,
      endpoint.instance,
      { executablePath: options.browserPath },
    )
    return { endpoint, browser, writer }
  } catch (error) {
    await browser?.close().catch(() => {})
    await writer.abort()
    throw error
  }
}

async function finishRuntime(runtime, fields, dependencies) {
  const { rejectedReferences = [], ...manifestFields } = fields
  await runtime.browser.close()
  await runtime.endpoint.finish()
  return runtime.writer.finish({
    ...manifestFields,
    source: await sourceManifest(runtime.endpoint, rejectedReferences, dependencies.lookup),
  })
}

export async function captureBrowserReference(options, dependencies = {}) {
  const pointSet = await (dependencies.loadPoints ?? loadParityPoints)()
  const rejectedReferences = await Promise.all((options.rejectedUrl ?? []).map((url) =>
    inspectEndpoint(url, { fetchImpl: dependencies.fetchImpl })))
  let runtime
  try {
    runtime = await openRuntime(options, pointSet.browserPoints.length, dependencies)
    const captured = await capturePoints(runtime.browser, runtime.writer, pointSet.browserPoints)
    const coverage = assertCompleteSuiteCoverage(captured.coverages, 'browser reference suite')
    const renderPlan = buildRenderPlan(captured.records)
    const rendered = await captureRenderEvidence(
      runtime.browser,
      captured.records,
      renderPlan,
      runtime.writer,
      (capture, point, label) => validateBrowserCapture(capture, point, label),
    )
    return await finishRuntime(runtime, {
      kind: 'quietmap-popup-browser-reference',
      created_at: new Date().toISOString(),
      rejectedReferences,
      points: pointManifest(pointSet),
      request_policy: requestPolicy(),
      coverage,
      browser: runtime.browser.metadata,
      render_plan: rendered,
    }, dependencies)
  } catch (error) {
    await runtime?.browser.close().catch(() => {})
    await runtime?.writer.abort().catch(() => {})
    throw error
  }
}

async function referenceEntriesByIndex(reference, pointSet) {
  const entries = []
  const attachments = new Map(reference.manifest.attachments?.map((item) => [item.file, item]))
  for await (const entry of reference.entries) {
    const point = pointSet.browserPoints[entry.index]
    if (!point) throw new Error(`reference has unexpected entry ${entry.index}`)
    const screenshot = entry.browser?.screenshot
    if (!screenshot || screenshot.sha256 !== attachments.get(screenshot.file)?.sha256) {
      throw new Error(`reference/${point.id}: missing or unverified point screenshot; recapture the reference`)
    }
    entries.push(validateReferenceEntry(entry, point))
  }
  if (entries.length !== pointSet.browserPoints.length) {
    throw new Error('browser reference entry count differs from current catalog')
  }
  return entries
}

function renderPlanWithoutCaptures(referencePlan) {
  return {
    schema_version: referencePlan.schema_version,
    coverage_basis: referencePlan.coverage_basis,
    layer_cover_point_ids: referencePlan.layer_cover_point_ids,
    items: referencePlan.items,
  }
}

export async function compareBrowserCandidate(options, dependencies = {}) {
  const pointSet = await (dependencies.loadPoints ?? loadParityPoints)()
  const reference = await openReferenceArtifact(options.reference, {
    browser_coordinates_sha256: pointSet.browserCoordinatesSha256,
  })
  if (reference.manifest.kind !== 'quietmap-popup-browser-reference') {
    throw new Error('compare-candidate requires a browser reference artifact')
  }
  const referenceEntries = await referenceEntriesByIndex(reference, pointSet)
  const acoustic = createComparisonAccumulator()
  const performance = createComparisonAccumulator()
  const visible = { text_equal: 0, screenshot_equal: 0, differences: [] }
  let runtime
  try {
    runtime = await openRuntime(options, pointSet.browserPoints.length, dependencies)
    assertComparableBrowser(reference.manifest.browser, runtime.browser.metadata)
    const captured = await capturePoints(
      runtime.browser,
      runtime.writer,
      pointSet.browserPoints,
      referenceEntries,
      (candidate, legacy, point) => {
        compareCanonical(canonicalizePopup(legacy.payload), canonicalizePopup(candidate.payload), point.id, acoustic)
        compareCanonical(
          performanceSnapshot(legacy.payload, legacy.elapsed_ms),
          performanceSnapshot(candidate.payload, candidate.elapsed_ms),
          point.id,
          performance,
        )
        compareVisibleEvidence(legacy, candidate, point, visible)
      },
    )
    const coverage = {
      reference: reference.manifest.coverage,
      candidate: assertCompleteSuiteCoverage(captured.coverages, 'browser candidate suite'),
    }
    const rendered = await captureRenderEvidence(
      runtime.browser,
      captured.records,
      renderPlanWithoutCaptures(reference.manifest.render_plan),
      runtime.writer,
      (capture, point, label) => validateBrowserCapture(capture, point, label),
    )
    const screenshots = await compareRenderEvidence(
      reference.manifest.render_plan,
      rendered,
      reference.readAttachment,
    )
    const acousticReport = comparisonReport(acoustic)
    const comparison = {
      schema_and_invariants: 'passed',
      exact_contract_pass: acousticReport.structural_differences.length === 0
        && acousticReport.categorical_differences.length === 0,
      numeric_verdict: 'diagnostic_only_no_acoustic_tolerance',
      screenshot_verdict: 'diagnostic_exact_hashes_no_pixel_tolerance',
      acoustic: acousticReport,
      performance: comparisonReport(performance),
      point_views: {
        exact_text_equal_count: visible.text_equal,
        exact_screenshot_equal_count: visible.screenshot_equal,
        difference_count: visible.differences.length,
        differences: visible.differences,
      },
      screenshots,
    }
    const result = await finishRuntime(runtime, {
      kind: 'quietmap-popup-browser-candidate',
      created_at: new Date().toISOString(),
      points: pointManifest(pointSet),
      request_policy: requestPolicy(),
      coverage,
      browser: runtime.browser.metadata,
      render_plan: rendered,
      comparison,
    }, dependencies)
    return { ...result, comparison }
  } catch (error) {
    await runtime?.browser.close().catch(() => {})
    await runtime?.writer.abort().catch(() => {})
    throw error
  }
}
