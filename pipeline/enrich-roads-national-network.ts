/** Apply one pinned national network-classification proxy to baked-country z9 roads. */

import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { nationalRoadProxySourceId } from './enrich-roads-national-policy.js'
import { pinnedRoadObservation, buildRoadLineVertexGrid, loadPinnedRoadLines, nearestRoadLine } from './lib/pinned-road-lines.js'
import { listPreparedSquares } from './lib/prepared-grid.js'
import { NATIONAL_ROAD_NETWORK_POLICIES, type NationalRoadLinePolicy } from './lib/road-national-network-policies/index.js'
import { parseRoadLoaderArguments, type RoadLoaderArguments } from './lib/road-loader-cli.js'
import { writeRoadAadt } from './lib/roads-arrow.js'
import { fanOutIfNeeded, parseShard, shardSquares } from './lib/square-pool.js'

export async function runNationalRoadNetworkPolicy(
  options: RoadLoaderArguments,
  policy: NationalRoadLinePolicy,
  shard = parseShard(undefined),
) {
  const loaded = loadPinnedRoadLines(options, policy.files)
  const grid = buildRoadLineVertexGrid(loaded.lines)
  const sourceId = nationalRoadProxySourceId(policy.country)
  const allSquares = listPreparedSquares(options.preparedDirectory, policy.bbox)
  if (allSquares.length === 0) throw new Error(`no ${policy.country} roads.arrow squares found under ${options.preparedDirectory}`)
  const squares = shardSquares(allSquares, shard)
  const match = (row: Parameters<NationalRoadLinePolicy['traffic']>[0]) => {
    if (!policy.coverage.has(row.roadClass)) return null
    const line = nearestRoadLine(row.midLat, row.midLon, grid, policy.radiusMetres, policy.acceptLine)
    if (!line) return null
    const traffic = policy.traffic(row, line)
    return traffic ? { ...traffic, ...pinnedRoadObservation(line, 'both-directions') } : null
  }
  const result = { country: policy.country, sourceId, sourceRows: loaded.sourceRows,
    sourceLines: loaded.lines.length, invalidGeometrySkipped: loaded.invalidGeometrySkipped,
    rows: 0, matched: 0, retracted: 0, skipped: 0, skippedForeign: 0,
    squares: squares.length, squaresUpdated: 0 }
  for (const square of squares) {
    const write = await writeRoadAadt(resolve(options.preparedDirectory, square, 'roads.arrow'), row => {
      const traffic = match(row)
      return traffic ? { ...traffic, sourceId } : null
    }, undefined, policy.coverage, {
      sourceIds: [sourceId],
      when: row => match(row) === null,
    })
    result.rows += write.rows
    result.matched += write.matched
    result.retracted += write.retracted
    result.skipped += write.skipped
    result.skippedForeign += write.skippedForeign
    if (write.updated) result.squaresUpdated++
  }
  return result
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const countryIndex = process.argv.indexOf('--country')
  const country = countryIndex >= 0 ? process.argv[countryIndex + 1]?.toUpperCase() : undefined
  const policy = country ? NATIONAL_ROAD_NETWORK_POLICIES.get(country) : undefined
  const shardIndex = process.argv.findIndex(arg => arg === '--shard' || arg.startsWith('--shard='))
  const shardArgument = shardIndex < 0 ? undefined : process.argv[shardIndex].startsWith('--shard=')
    ? process.argv[shardIndex].slice('--shard='.length) : process.argv[shardIndex + 1]
  const loaderArgs = process.argv.slice(2).filter((_, index, argv) =>
    !['--country', '--shard'].includes(argv[index - 1]) &&
    !['--country', '--shard'].includes(argv[index]) && !argv[index].startsWith('--shard='))
  if (!policy) {
    console.error(`--country must be one of ${[...NATIONAL_ROAD_NETWORK_POLICIES.keys()].join(',')}`)
    process.exitCode = 1
  } else {
    Promise.resolve().then(async () => {
      const options = parseRoadLoaderArguments(loaderArgs, 'enrich-roads-national-network.ts')
      if (shardIndex >= 0 && shardArgument === undefined) throw new Error('--shard requires index/count')
      const shard = parseShard(shardArgument)
      // India uses about 2 GiB resident; reserve room for each source grid and decoded square.
      if (await fanOutIfNeeded(4 * 1024 ** 3)) return
      console.log(JSON.stringify(await runNationalRoadNetworkPolicy(options, policy, shard)))
    })
      .catch((error: unknown) => {
        console.error(error instanceof Error ? error.message : error)
        process.exitCode = 1
      })
  }
}
