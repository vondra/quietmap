#!/usr/bin/env node
//! Capture and compare cohort-consistent worldwide popup JSON diagnostics.

import { createHash } from 'node:crypto'
import { lookup } from 'node:dns/promises'
import { mkdir, writeFile } from 'node:fs/promises'
import { dirname, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { parseArgs } from 'node:util'
import { createCaptureWriter, openReferenceArtifact } from './popup-parity/artifact.mjs'
import {
  canonicalizePopup,
  compareCanonical,
  comparisonReport,
  createComparisonAccumulator,
  performanceSnapshot,
} from './popup-parity/contract.mjs'
import {
  REQUEST_CONCURRENCY,
  REQUEST_INTERVAL_MS,
  consumeRateLimited,
  inspectEndpoint,
  openEndpointSession,
} from './popup-parity/endpoint.mjs'
import {
  CURATED_POINT_COUNT,
  GENERATED_POINT_COUNT,
  loadParityPoints,
} from './popup-parity/points.mjs'
import {
  assertCompleteSuiteCoverage,
  payloadCoverage,
  validatePopupPayload,
} from './popup-parity/payload-schema.mjs'

const USAGE = `Usage:
  node scripts/popup-parity.mjs capture-reference --url <origin-or-popup-url> --output <new-directory> [--instance <id>] [--rejected-url <url>]
  node scripts/popup-parity.mjs compare-candidate --url <origin-or-popup-url> --reference <manifest-or-directory> --output <new-report.json> [--instance <id>]

The 264 requests use the default (not full=1) popup and start at 4 requests/s.
This command is an API diagnostic; browser-driven popup capture is the release acceptance.`

function pointRecord(point) {
  return Object.fromEntries(
    ['id', 'lat', 'lng', 'group', 'role', 'mode', 'host', 'anchor_type']
      .filter((key) => Object.hasOwn(point, key))
      .map((key) => [key, point[key]]),
  )
}

function bodySha256(body) {
  return createHash('sha256').update(body).digest('hex')
}

function progress(done, total, label) {
  if (done === total || done % 10 === 0) process.stderr.write(`${label}: ${done}/${total}\n`)
}

function pointManifest(points, hashes) {
  return {
    curated_count: CURATED_POINT_COUNT,
    equal_area_count: GENERATED_POINT_COUNT,
    total_count: points.length,
    hashes,
  }
}

function requestPolicy() {
  return {
    mode: 'default',
    full: false,
    starts_per_second: 1000 / REQUEST_INTERVAL_MS,
    concurrency: REQUEST_CONCURRENCY,
    acoustic_tolerances: null,
  }
}

export async function captureReference(options, dependencies = {}) {
  const pointSet = await (dependencies.loadPoints ?? loadParityPoints)()
  const rejectedReferences = await Promise.all((options.rejectedUrl ?? []).map((url) =>
    inspectEndpoint(url, { fetchImpl: dependencies.fetchImpl })))
  const session = await (dependencies.openSession ?? openEndpointSession)(options.url, {
    expectedInstance: options.instance,
    fetchImpl: dependencies.fetchImpl,
  })
  const writer = await (dependencies.createWriter ?? createCaptureWriter)(
    options.output,
    pointSet.points.length,
  )
  const coverages = new Array(pointSet.points.length)
  let completed = 0
  try {
    await consumeRateLimited(
      pointSet.points,
      async (point) => {
        const response = await session.query(point)
        validatePopupPayload(response.payload, point, `reference/${point.id}`)
        return response
      },
      async (response, point, index) => {
        coverages[index] = payloadCoverage(response.payload)
        await writer.append(index, {
          schema_version: 1,
          index,
          point: pointRecord(point),
          instance: session.instance,
          http_status: response.http_status,
          content_type: response.content_type,
          elapsed_ms: response.elapsed_ms,
          body_sha256: bodySha256(response.body),
          payload: response.payload,
        })
        completed += 1
        progress(completed, pointSet.points.length, 'capture-reference')
      },
      dependencies.rateOptions,
    )
    const coverage = assertCompleteSuiteCoverage(coverages, 'reference suite')
    await session.finish()
    const hostname = new URL(session.popupUrl).hostname
    const resolvedAddresses = [...new Set(
      (await (dependencies.lookup ?? lookup)(hostname, { all: true })).map(({ address }) => address),
    )].sort()
    const result = await writer.finish({
      created_at: new Date().toISOString(),
      source: {
        url: session.popupUrl,
        hostname,
        resolved_addresses: resolvedAddresses,
        instance: session.instance,
        cohort: session.cohort,
        rejected_references: rejectedReferences,
      },
      points: pointManifest(pointSet.points, pointSet.hashes),
      request_policy: requestPolicy(),
      coverage,
    })
    return result
  } catch (error) {
    await writer.abort()
    throw error
  }
}

function assertEntryPoint(entry, point) {
  for (const key of ['id', 'lat', 'lng']) {
    if (String(entry.point[key]) !== String(point[key])) {
      throw new Error(`reference entry ${entry.index}: point ${key} differs from current catalog`)
    }
  }
}

async function writeImmutableReport(path, report) {
  const destination = resolve(path)
  await mkdir(dirname(destination), { recursive: true })
  await writeFile(destination, `${JSON.stringify(report, null, 2)}\n`, { flag: 'wx' })
  return destination
}

export async function compareCandidate(options, dependencies = {}) {
  const pointSet = await (dependencies.loadPoints ?? loadParityPoints)()
  const reference = await openReferenceArtifact(options.reference, pointSet.hashes)
  if (reference.manifest.points.total_count !== pointSet.points.length) {
    throw new Error('reference manifest point count differs from current catalog')
  }
  const session = await (dependencies.openSession ?? openEndpointSession)(options.url, {
    expectedInstance: options.instance,
    fetchImpl: dependencies.fetchImpl,
  })
  const referenceCoverage = new Array(pointSet.points.length)
  const candidateCoverage = new Array(pointSet.points.length)
  const acousticState = createComparisonAccumulator()
  const performanceState = createComparisonAccumulator()
  let completed = 0

  await consumeRateLimited(
    reference.entries,
    async (entry, index) => {
      const point = pointSet.points[index]
      assertEntryPoint(entry, point)
      validatePopupPayload(entry.payload, point, `reference/${point.id}`)
      const candidate = await session.query(point)
      validatePopupPayload(candidate.payload, point, `candidate/${point.id}`)
      return { entry, candidate, point }
    },
    ({ entry, candidate, point }, _referenceEntry, index) => {
      referenceCoverage[index] = payloadCoverage(entry.payload)
      candidateCoverage[index] = payloadCoverage(candidate.payload)
      compareCanonical(
        canonicalizePopup(entry.payload),
        canonicalizePopup(candidate.payload),
        point.id,
        acousticState,
      )
      compareCanonical(
        performanceSnapshot(entry.payload, entry.elapsed_ms),
        performanceSnapshot(candidate.payload, candidate.elapsed_ms),
        point.id,
        performanceState,
      )
      completed += 1
      progress(completed, pointSet.points.length, 'compare-candidate')
    },
    dependencies.rateOptions,
  )
  const coverage = {
    reference: assertCompleteSuiteCoverage(referenceCoverage, 'reference suite'),
    candidate: assertCompleteSuiteCoverage(candidateCoverage, 'candidate suite'),
  }
  await session.finish()
  const report = {
    schema_version: 1,
    kind: 'quietmap-popup-api-diagnostic',
    created_at: new Date().toISOString(),
    acceptance_scope: 'diagnostic_only_browser_popup_capture_required',
    reference: {
      manifest: reference.manifestPath,
      url: reference.manifest.source.url,
      instance: reference.manifest.source.instance,
      cohort: reference.manifest.source.cohort,
    },
    candidate: {
      url: session.popupUrl,
      instance: session.instance,
      cohort: session.cohort,
    },
    points: pointManifest(pointSet.points, pointSet.hashes),
    coverage,
    acoustic: comparisonReport(acousticState),
    performance: comparisonReport(performanceState),
  }
  return { report, reportPath: await writeImmutableReport(options.output, report) }
}

function cliOptions(argv) {
  const parsed = parseArgs({
    args: argv,
    allowPositionals: true,
    strict: true,
    options: {
      url: { type: 'string' },
      output: { type: 'string' },
      reference: { type: 'string' },
      instance: { type: 'string' },
      'rejected-url': { type: 'string', multiple: true },
      help: { type: 'boolean', short: 'h' },
    },
  })
  if (parsed.values.help) return { help: true }
  const [command, ...extra] = parsed.positionals
  if (extra.length || !['capture-reference', 'compare-candidate'].includes(command)) {
    throw new Error('expected capture-reference or compare-candidate')
  }
  if (!parsed.values.url || !parsed.values.output) throw new Error('--url and --output are required')
  if (command === 'compare-candidate' && !parsed.values.reference) {
    throw new Error('--reference is required for compare-candidate')
  }
  if (command === 'capture-reference' && parsed.values.reference) {
    throw new Error('--reference is only valid for compare-candidate')
  }
  return {
    command,
    ...parsed.values,
    rejectedUrl: parsed.values['rejected-url'] ?? [],
  }
}

export async function main(argv = process.argv.slice(2)) {
  let options
  try {
    options = cliOptions(argv)
  } catch (error) {
    process.stderr.write(`${error.message}\n\n${USAGE}\n`)
    return 2
  }
  if (options.help) {
    process.stdout.write(`${USAGE}\n`)
    return 0
  }
  try {
    const result = options.command === 'capture-reference'
      ? await captureReference(options)
      : await compareCandidate(options)
    const summary = options.command === 'capture-reference'
      ? {
          manifest: result.manifestPath,
          entries: result.manifest.artifact.entry_count,
          compressed_sha256: result.manifest.artifact.compressed_sha256,
          coordinates_sha256: result.manifest.points.hashes.all_coordinates_sha256,
        }
      : {
          report: result.reportPath,
          structural_differences: result.report.acoustic.structural_differences.length,
          categorical_differences: result.report.acoustic.categorical_differences.length,
          numeric_verdict: 'diagnostic_only',
        }
    process.stdout.write(`${JSON.stringify(summary, null, 2)}\n`)
    return 0
  } catch (error) {
    process.stderr.write(`popup parity failed: ${error.stack ?? error.message}\n`)
    return 1
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.exitCode = await main()
}
