/** Contract tests for explicit national road-loader paths and cache modes. */

import assert from 'node:assert/strict'
import test from 'node:test'
import { parseRoadLoaderArguments } from './road-loader-cli.js'

test('road loader paths are explicit and cache modes are mutually exclusive', () => {
  assert.throws(() => parseRoadLoaderArguments([], 'loader.ts'), /usage/)
  assert.throws(() => parseRoadLoaderArguments([
    '--prepared-dir', 'prepared', '--enrichment-dir', 'enrichment',
    '--enrich-only', '--force-download',
  ], 'loader.ts'), /mutually exclusive/)
  const parsed = parseRoadLoaderArguments([
    '--prepared-dir', 'prepared', '--enrichment-dir', 'enrichment', '--enrich-only',
  ], 'loader.ts')
  assert.ok(parsed.preparedDirectory.endsWith('/prepared'))
  assert.ok(parsed.enrichmentDirectory.endsWith('/enrichment'))
  assert.equal(parsed.enrichOnly, true)
})
