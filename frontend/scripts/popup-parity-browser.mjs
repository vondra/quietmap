#!/usr/bin/env node
//! CLI for browser-driven worldwide popup reference capture and comparison.

import { pathToFileURL } from 'node:url'
import { parseArgs } from 'node:util'
import {
  captureBrowserReference,
  compareBrowserCandidate,
} from './popup-parity/browser-runner.mjs'

const USAGE = `Usage:
  node scripts/popup-parity-browser.mjs capture-reference --url <origin-or-popup-url> --output <new-directory> [--instance <id>] [--rejected-url <url>] [--browser-path <path>]
  node scripts/popup-parity-browser.mjs compare-candidate --url <origin-or-popup-url> --reference <browser-reference> --output <new-directory> [--instance <id>] [--browser-path <path>]

The 200 requests are real MapLibre canvas-centre clicks at distinct locations:
deduplicated curated anchors plus deterministic equal-area points. Popup starts are
sequential and no faster than 4/s. Every point includes an unmodified live-web
screenshot; additional views exercise all seven layer controls. Numeric and
screenshot differences are reported without invented acceptance tolerances.`

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
      'browser-path': { type: 'string' },
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
    browserPath: parsed.values['browser-path'],
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
      ? await captureBrowserReference(options)
      : await compareBrowserCandidate(options)
    process.stdout.write(`${JSON.stringify({
      manifest: result.manifestPath,
      entries: result.manifest.artifact.entry_count,
      compressed_sha256: result.manifest.artifact.compressed_sha256,
      coordinates_sha256: result.manifest.points.hashes.browser_coordinates_sha256,
      schema_and_invariants: result.comparison?.schema_and_invariants ?? 'passed',
      numeric_verdict: result.comparison?.numeric_verdict ?? 'not_compared',
    }, null, 2)}\n`)
    return 0
  } catch (error) {
    process.stderr.write(`browser popup parity failed: ${error.stack ?? error.message}\n`)
    return 1
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.exitCode = await main()
}
