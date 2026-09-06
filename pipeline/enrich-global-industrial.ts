/** Classify current industrial Arrow sites from the five retained global registry sources. */

import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { parseArgs } from 'node:util'
import { loadGlobalIndustrialSources } from './lib/industrial-global-source.js'
import { enrichIndustrialFacilities } from './lib/industrial-arrow.js'

async function main(): Promise<void> {
  const { values } = parseArgs({ options: {
    'prepared-dir': { type: 'string' }, 'enrichment-dir': { type: 'string' },
  } })
  if (!values['prepared-dir'] || !values['enrichment-dir']) {
    throw new Error('usage: enrich-global-industrial.ts --prepared-dir PREPARED_YEAR_DIR --enrichment-dir GLOBAL_ENRICHMENT_DIR')
  }
  const source = loadGlobalIndustrialSources(resolve(values['enrichment-dir']))
  console.log(JSON.stringify({ sources: source.receipts }))
  console.log(JSON.stringify(await enrichIndustrialFacilities(resolve(values['prepared-dir']), source.facilities, source.resetSourceIds)))
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error: unknown) => { console.error(error); process.exitCode = 1 })
}
