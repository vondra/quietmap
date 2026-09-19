/** Apply one pinned national network-classification proxy to baked-country z9 roads. */

import { nationalRoadProxySourceId } from './enrich-roads-national-policy.js'
import { pinnedRoadObservation, buildRoadLineVertexGrid, loadPinnedRoadLines, nearestRoadLine } from './lib/pinned-road-lines.js'
import { NATIONAL_ROAD_NETWORK_POLICIES, type NationalRoadLinePolicy } from './lib/road-national-network-policies/index.js'
import { parseRoadLoaderArguments, type RoadLoaderArguments } from './lib/road-loader-cli.js'
import { writeRoadAadt } from './lib/roads-arrow.js'
import { runRoadSquaresCli, writeNationalRoadSquares } from './lib/square-pool.js'

export async function runNationalRoadNetworkPolicy(options: RoadLoaderArguments, policy: NationalRoadLinePolicy) {
  const loaded = loadPinnedRoadLines(options, policy.files)
  const grid = buildRoadLineVertexGrid(loaded.lines)
  const sourceId = nationalRoadProxySourceId(policy.country)
  const match = (row: Parameters<NationalRoadLinePolicy['traffic']>[0]) => {
    if (!policy.coverage.has(row.roadClass)) return null
    const line = nearestRoadLine(row.midLat, row.midLon, grid, policy.radiusMetres, policy.acceptLine)
    if (!line) return null
    const traffic = policy.traffic(row, line)
    return traffic ? { ...traffic, ...pinnedRoadObservation(line, 'both-directions') } : null
  }
  const counters = await writeNationalRoadSquares(options.preparedDirectory, policy.bbox, policy.country, {}, path =>
    writeRoadAadt(path, row => {
      const traffic = match(row)
      return traffic ? { ...traffic, sourceId } : null
    }, undefined, policy.coverage, {
      sourceIds: [sourceId],
      when: row => match(row) === null,
    }))
  return { country: policy.country, sourceId, sourceRows: loaded.sourceRows,
    sourceLines: loaded.lines.length, invalidGeometrySkipped: loaded.invalidGeometrySkipped, ...counters }
}

runRoadSquaresCli(import.meta.url, async () => {
  const countryIndex = process.argv.indexOf('--country')
  const policy = countryIndex < 0 ? undefined : NATIONAL_ROAD_NETWORK_POLICIES.get(process.argv[countryIndex + 1]?.toUpperCase())
  if (!policy) throw new Error(`--country must be one of ${[...NATIONAL_ROAD_NETWORK_POLICIES.keys()].join(',')}`)
  const loaderArguments = process.argv.slice(2).filter((_, index) => index + 2 !== countryIndex && index + 1 !== countryIndex)
  return runNationalRoadNetworkPolicy(
    parseRoadLoaderArguments(loaderArguments, 'enrich-roads-national-network.ts'), policy)
})
