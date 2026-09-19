/** Strict shared path and cache-mode arguments for national road loaders. */

import { basename, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { parseArgs } from 'node:util'
import { ownSquareShard, runRoadSquaresCli } from './square-pool.js'

export interface RoadLoaderArguments {
  preparedDirectory: string
  enrichmentDirectory: string
  enrichOnly: boolean
  forceDownload: boolean
}

export function parseRoadLoaderArguments(
  argv: readonly string[],
  executable: string,
): RoadLoaderArguments {
  const { values } = parseArgs({
    args: [...argv],
    strict: true,
    allowPositionals: false,
    options: {
      'prepared-dir': { type: 'string' },
      'enrichment-dir': { type: 'string' },
      'enrich-only': { type: 'boolean', default: false },
      'force-download': { type: 'boolean', default: false },
    },
  })
  if (!values['prepared-dir'] || !values['enrichment-dir']) {
    throw new Error(`usage: ${executable} --prepared-dir DIR --enrichment-dir DIR [--enrich-only|--force-download]`)
  }
  if (values['enrich-only'] && values['force-download']) {
    throw new Error('--enrich-only and --force-download are mutually exclusive')
  }
  return {
    preparedDirectory: resolve(values['prepared-dir']),
    enrichmentDirectory: resolve(values['enrichment-dir']),
    // A shard child reads the cache its parent has already loaded; it never downloads again.
    enrichOnly: values['enrich-only'] || ownSquareShard !== null,
    forceDownload: values['force-download'] && ownSquareShard === null,
  }
}

export function runRoadLoaderCli(moduleUrl: string, run: (options: RoadLoaderArguments) => Promise<object>): void {
  runRoadSquaresCli(moduleUrl, async () =>
    run(parseRoadLoaderArguments(process.argv.slice(2), basename(fileURLToPath(moduleUrl)))))
}
