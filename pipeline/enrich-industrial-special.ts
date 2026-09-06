/** Apply the admitted ten special-national sources to country-owned industrial rows. */

import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { parseArgs } from 'node:util'
import { loadSpecialIndustrialSources } from './lib/industrial-special-source.js'
import { enrichIndustrialFacilities } from './lib/industrial-arrow.js'

async function main(): Promise<void> {
  const { values } = parseArgs({ options: {
    'prepared-dir': { type: 'string' }, 'enrichment-dir': { type: 'string' }, boundaries: { type: 'string' },
  } })
  if (!values['prepared-dir'] || !values['enrichment-dir'] || !values.boundaries) {
    throw new Error('usage: enrich-industrial-special.ts --prepared-dir PREPARED_YEAR_DIR --enrichment-dir ENRICHMENT_YEAR_DIR --boundaries CGAZ_GEOJSON')
  }
  const source = loadSpecialIndustrialSources(resolve(values['enrichment-dir']), resolve(values.boundaries))
  console.log(JSON.stringify({ sources: source.receipts }))
  console.log(JSON.stringify(await enrichIndustrialFacilities(resolve(values['prepared-dir']), source.facilities,
    source.resetSourceIds, source.ownership)))
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error: unknown) => { console.error(error); process.exitCode = 1 })
}
