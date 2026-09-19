/** Enrich Thailand roads from pinned DRR counts and explicit DOH corridor policies. */

import { roadObservation } from './lib/road-observation.js'
import { runRoadLoaderCli, type RoadLoaderArguments } from './lib/road-loader-cli.js'
import { loadThailandDrrSource, thailandDrrTraffic, type ThailandDrrSource } from './lib/roads-th-source.js'
import { inBbox } from './lib/spatial.js'
import { SOURCE_ID_TH_ROAD_CLASSIFICATION_FALLBACK, SOURCE_ID_TH_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { isSlipRoadClass, writeRoadAadt, type RoadRow } from './lib/roads-arrow.js'
import { writeNationalRoadSquares } from './lib/square-pool.js'

const THAILAND_BBOX = [5.5, 97.3, 20.5, 105.7] as const
const BANGKOK_BBOX = [13.5, 100.3, 14.2, 100.9] as const
const MOTORWAY_AADT: Readonly<Record<string, number>> = {
  '7': 120_000, '9': 100_000, '81': 50_000, '82': 40_000,
}
const TRUNK_AADT: Readonly<Record<string, { rural: number; bangkok: number }>> = {
  '1': { rural: 35_000, bangkok: 95_000 }, '2': { rural: 30_000, bangkok: 85_000 },
  '3': { rural: 25_000, bangkok: 75_000 }, '4': { rural: 28_000, bangkok: 80_000 },
  '11': { rural: 20_000, bangkok: 40_000 }, '12': { rural: 15_000, bangkok: 30_000 },
  '22': { rural: 18_000, bangkok: 35_000 }, '24': { rural: 15_000, bangkok: 25_000 },
  '32': { rural: 45_000, bangkok: 85_000 }, '33': { rural: 22_000, bangkok: 40_000 },
  '34': { rural: 80_000, bangkok: 120_000 }, '35': { rural: 90_000, bangkok: 130_000 },
  '41': { rural: 20_000, bangkok: 35_000 }, '304': { rural: 15_000, bangkok: 35_000 },
}

function policyTraffic(total: number, bangkok: boolean) {
  const split = bangkok
    ? { light: .60, medium: .08, heavy: .07, moto: .25 }
    : { light: .62, medium: .10, heavy: .13, moto: .15 }
  return { light: Math.round(total * split.light), medium: Math.round(total * split.medium),
    heavy: Math.round(total * split.heavy), moto: Math.round(total * split.moto) }
}

export function matchThailandRoad(row: RoadRow, source: ThailandDrrSource) {
  const ref = row.ref?.trim()
  // A slip road inherits the mainline ref; the route's count was never taken on it.
  if (!ref || isSlipRoadClass(row.roadClass)) return null
  const direct = source.records.get(ref)
  if (direct) return { ...roadObservation(direct.roadCode, 'both-directions'), kind: 'drr' as const, ...thailandDrrTraffic(direct) }
  const bangkok = inBbox(row.midLat, row.midLon, BANGKOK_BBOX)
  for (const token of ref.split(/[;,]/).map(value => value.trim()).filter(Boolean)) {
    const motorway = MOTORWAY_AADT[token]
    if (motorway) return { ...roadObservation({ policy: 'motorway', token, bangkok }, 'both-directions'), kind: 'motorway' as const, ...policyTraffic(motorway, bangkok) }
  }
  for (const token of ref.split(/[;,]/).map(value => value.trim()).filter(Boolean)) {
    const trunk = TRUNK_AADT[token]
    if (trunk) return { ...roadObservation({ policy: 'trunk', token, bangkok }, 'both-directions'), kind: 'trunk' as const,
      ...policyTraffic(bangkok ? trunk.bangkok : trunk.rural, bangkok) }
  }
  return null
}

export async function enrichThailandRoads(preparedDirectory: string, source: ThailandDrrSource) {
  const tally = { matchedDrr: 0, matchedMotorway: 0, matchedTrunk: 0 }
  const match = (row: RoadRow) => matchThailandRoad(row, source)
  const counters = await writeNationalRoadSquares(preparedDirectory, THAILAND_BBOX, 'Thailand', tally, path =>
    writeRoadAadt(path, row => {
      const traffic = match(row)
      return traffic ? { countBasis: traffic.countBasis, observationId: traffic.observationId, light: traffic.light, medium: traffic.medium, heavy: traffic.heavy,
        moto: traffic.moto, sourceId: traffic.kind === 'drr' ? SOURCE_ID_TH_NATIONAL_ROADS : SOURCE_ID_TH_ROAD_CLASSIFICATION_FALLBACK } : null
    }, (_row, _index, traffic) => {
      const kind = match(_row)?.kind
      if (kind === 'drr') tally.matchedDrr++
      else if (kind === 'motorway') tally.matchedMotorway++
      else if (kind === 'trunk') tally.matchedTrunk++
    }, undefined, { sourceIds: [SOURCE_ID_TH_NATIONAL_ROADS, SOURCE_ID_TH_ROAD_CLASSIFICATION_FALLBACK], when: row => { const traffic = match(row); return traffic === null ||
        (traffic.kind === 'drr' ? SOURCE_ID_TH_NATIONAL_ROADS : SOURCE_ID_TH_ROAD_CLASSIFICATION_FALLBACK) !== row.existingSourceId } }))
  return { ...counters, ...tally }
}

export async function runThailandRoadEnrichment(options: RoadLoaderArguments) {
  const source = loadThailandDrrSource(options)
  return { sourceRows: source.sourceRows, records: source.records.size,
    unavailableTrafficSkipped: source.unavailableTrafficSkipped,
    invalidClassCountsSkipped: source.invalidClassCountsSkipped,
    supersededRecords: source.supersededRecords,
    ...await enrichThailandRoads(options.preparedDirectory, source) }
}

runRoadLoaderCli(import.meta.url, runThailandRoadEnrichment)
