/** Stamp CZPTT graph counts and the explicitly owned Czech timetable-silent residual. */

import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { parseArgs } from 'node:util'
import { readCzpttSource } from './lib/railway-cz-source.js'
import { enrichZ9RailwaysByGraphWalk } from './lib/rail-walk-enrich.js'
import { SOURCE_ID_CZ_SZCD_GTFS, SOURCE_ID_CZ_TIMETABLE_SILENT } from './lib/source-ids.generated.js'

export async function enrichCzechRailways(sourceDirectory: string, preparedDirectory: string) {
  const { pairs, ...source } = readCzpttSource(sourceDirectory)
  const walk = await enrichZ9RailwaysByGraphWalk({
    preparedDirectory,
    bbox: [48, 11.5, 51.5, 19.5],
    countryIso: 'CZ', sourceId: SOURCE_ID_CZ_SZCD_GTFS, pairs,
    // Dev1's explicit CZ residual: no scheduled service is not measured freight or absolute silence.
    silentResidual: { sourceId: SOURCE_ID_CZ_TIMETABLE_SILENT, passenger: 2, freight: 1 },
  })
  return { source: { ...source, stationPairs: pairs.length }, walk }
}

async function main(): Promise<void> {
  const { values } = parseArgs({ options: {
    'source-dir': { type: 'string' }, 'prepared-dir': { type: 'string' },
  }, strict: true, allowPositionals: false })
  if (!values['source-dir'] || !values['prepared-dir']) {
    throw new Error('usage: enrich-railways-cz.ts --source-dir CZ_CACHE_DIR --prepared-dir PREPARED_YEAR_DIR')
  }
  console.log(JSON.stringify(await enrichCzechRailways(resolve(values['source-dir']), resolve(values['prepared-dir']))))
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error: unknown) => { console.error(error); process.exitCode = 1 })
}
