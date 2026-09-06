/** Refine CZ/ES OSM building floors and use before service-tree and final structures. */

import { existsSync, mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { parseArgs } from 'node:util'
import { listPreparedSquares } from './lib/prepared-grid.js'
import { shouldOverwrite } from './lib/sources.js'
import { writeBuildingEnrichment } from './lib/buildings-arrow.js'
import { NATIONAL_BUILDING_SOURCES, indexNationalBuildings, type NationalBuildingIndex } from './lib/buildings-national-source.js'

export async function enrichNationalBuildings(preparedDirectory: string, index: NationalBuildingIndex) {
  const squares = new Set([
    ...listPreparedSquares(preparedDirectory, index.source.bbox, 'buildings.arrow'),
    ...listPreparedSquares(preparedDirectory, index.source.bbox, 'roads.arrow'),
    ...listPreparedSquares(preparedDirectory, index.source.bbox, 'structures.arrow'),
  ])
  if (!squares.size) throw new Error(`${preparedDirectory}: no prepared building scope for ${index.source.country}`)
  // A road/structure owner with a missing building table is not evidence of no buildings.
  for (const square of squares) {
    if (!existsSync(resolve(preparedDirectory, square, 'buildings.arrow'))) {
      throw new Error(`${square}: missing buildings.arrow before national enrichment`)
    }
  }
  const result = { country: index.source.country, rows: 0, matched: 0, floorsAdded: 0,
    typesChanged: 0, typeDowngradesBlocked: 0, squares: squares.size, squaresUpdated: 0 }
  for (const square of [...squares].sort()) {
    const written = await writeBuildingEnrichment(resolve(preparedDirectory, square, 'buildings.arrow'), row => {
      if (!shouldOverwrite(row.existingSourceId, index.source.sourceId)) return null
      if (index.source.country === 'ES' && row.floors > 0) return null
      const point = index.nearest(row.lat, row.lon)
      if (!point) return null
      return { sourceId: index.source.sourceId,
        floors: row.floors === 0 && point.floors > 0 ? point.floors : undefined,
        buildingType: point.buildingType ?? undefined }
    })
    for (const key of ['rows', 'matched', 'floorsAdded', 'typesChanged', 'typeDowngradesBlocked'] as const) result[key] += written[key]
    if (written.updated) result.squaresUpdated++
  }
  return result
}

async function main(): Promise<void> {
  const { values } = parseArgs({ options: {
    'prepared-dir': { type: 'string' }, 'enrichment-dir': { type: 'string' },
  } })
  if (!values['prepared-dir'] || !values['enrichment-dir']) {
    throw new Error('usage: enrich-buildings-national.ts --prepared-dir PREPARED_YEAR_DIR --enrichment-dir ENRICHMENT_YEAR_DIR')
  }
  const work = mkdtempSync(resolve(tmpdir(), 'qm-national-buildings-'))
  const indexes: NationalBuildingIndex[] = []
  try {
    // All selected caches must pass admission before any prepared row changes.
    for (const source of NATIONAL_BUILDING_SOURCES) {
      indexes.push(await indexNationalBuildings(resolve(values['enrichment-dir'], source.file),
        resolve(work, `${source.country}.sqlite`), source))
    }
    for (const index of indexes) {
      console.log(JSON.stringify({ country: index.source.country, source: index.receipt }))
      console.log(JSON.stringify(await enrichNationalBuildings(resolve(values['prepared-dir']), index)))
    }
  } finally {
    for (const index of indexes) index.close()
    rmSync(work, { recursive: true, force: true })
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error: unknown) => { console.error(error); process.exitCode = 1 })
}
