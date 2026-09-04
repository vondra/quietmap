/** Strict shared path and cache-mode arguments for national road loaders. */

import { resolve } from 'node:path'
import { parseArgs } from 'node:util'

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
    enrichOnly: values['enrich-only'],
    forceDownload: values['force-download'],
  }
}
