/** Enrich CN or IN z9 railways from canonical national and metro line sources. */

import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { parseArgs } from 'node:util'
import { listPreparedSquares, type PreparedBbox } from './lib/prepared-grid.js'
import {
  buildSpatialRailGrid, loadSpatialRailSource, nearestSpatialRailFeature,
  spatialRailFeatureKinds, trafficForSpatialRailFeature, type SpatialRailCountry,
} from './lib/railway-spatial-source.js'
import { writeRailwayTraffic } from './lib/railways-arrow.js'
import { SOURCE_ID_CN_NATIONAL_RAILWAY, SOURCE_ID_IN_NATIONAL_RAILWAY } from './lib/source-ids.generated.js'

const PROFILES: Record<SpatialRailCountry, { bbox: PreparedBbox; sourceId: number }> = {
  CN: { bbox: [18, 73, 54, 135.5], sourceId: SOURCE_ID_CN_NATIONAL_RAILWAY },
  IN: { bbox: [6.5, 68, 37, 98], sourceId: SOURCE_ID_IN_NATIONAL_RAILWAY },
}

export interface SpatialRailwayResult {
  country: SpatialRailCountry
  nationalFeatures: number
  metroFeatures: number
  squares: number
  rows: number
  matched: number
  retracted: number
  skippedService: number
  skippedForeign: number
  skippedPriority: number
  squaresUpdated: number
}

export async function enrichSpatialRailwayCountry(options: {
  sourceDirectory: string
  preparedDirectory: string
  country: SpatialRailCountry
}): Promise<SpatialRailwayResult> {
  const profile = PROFILES[options.country]
  const source = loadSpatialRailSource(resolve(options.sourceDirectory), options.country)
  const grids = {
    national: buildSpatialRailGrid(source.national),
    metro: buildSpatialRailGrid(source.metro),
  }
  const prepared = resolve(options.preparedDirectory)
  const squares = listPreparedSquares(prepared, profile.bbox, 'railways.arrow')
  if (squares.length === 0) throw new Error(`no ${options.country} railways.arrow source squares found under ${prepared}`)
  const result: SpatialRailwayResult = {
    country: options.country,
    nationalFeatures: source.national.length,
    metroFeatures: source.metro.length,
    squares: squares.length,
    rows: 0,
    matched: 0,
    retracted: 0,
    skippedService: 0,
    skippedForeign: 0,
    skippedPriority: 0,
    squaresUpdated: 0,
  }
  for (const square of squares) {
    const write = await writeRailwayTraffic(
      resolve(prepared, square, 'railways.arrow'),
      row => {
        const kinds = spatialRailFeatureKinds(options.country, row.railType)
        const feature = nearestSpatialRailFeature(row, kinds.map(kind => grids[kind]))
        if (!feature) return null
        return { ...trafficForSpatialRailFeature(options.country, feature, row.midLat, row.midLon), sourceId: profile.sourceId }
      },
      undefined,
      {
        allowedCountryIsos: [options.country],
        retract: { sourceIds: [profile.sourceId], when: () => true },
      },
    )
    result.rows += write.rows
    result.matched += write.matched
    result.retracted += write.retracted
    result.skippedService += write.skippedService
    result.skippedForeign += write.skippedForeign
    result.skippedPriority += write.skippedPriority
    result.squaresUpdated += Number(write.updated)
  }
  return result
}

function cliOptions(argv: readonly string[]): {
  sourceDirectory: string
  preparedDirectory: string
  country: SpatialRailCountry
} {
  const { values } = parseArgs({
    args: [...argv], strict: true, allowPositionals: false,
    options: {
      'source-dir': { type: 'string' },
      'prepared-dir': { type: 'string' },
      country: { type: 'string' },
    },
  })
  if (!values['source-dir'] || !values['prepared-dir'] ||
      (values.country !== 'CN' && values.country !== 'IN')) {
    throw new Error('usage: enrich-railway-spatial.ts --source-dir DIR --prepared-dir DIR --country CN|IN')
  }
  return {
    sourceDirectory: values['source-dir'],
    preparedDirectory: values['prepared-dir'],
    country: values.country,
  }
}

async function main(): Promise<void> {
  console.log(JSON.stringify(await enrichSpatialRailwayCountry(cliOptions(process.argv.slice(2)))))
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error: unknown) => {
    console.error(error instanceof Error ? error.message : error)
    process.exitCode = 1
  })
}
