/** Apply one registered country policy to baked-country z9 roads. */

import { resolve } from 'node:path'
import { parseArgs } from 'node:util'
import { pathToFileURL } from 'node:url'
import { listPreparedSquares } from './lib/prepared-grid.js'
import { DATASETS } from './lib/enrichment-datasets.js'
import { NATIONAL_ROAD_POLICIES, type NationalRoadPolicy } from './lib/road-national-policies/index.js'
import { writeRoadAadt } from './lib/roads-arrow.js'
import { SOURCES_BY_KEY } from './lib/sources.js'

export function nationalRoadProxySourceId(country: string): number {
  const key = `${country.toLowerCase()}-national-roads`
  const source = SOURCES_BY_KEY.get(key)
  const dataset = DATASETS.find(candidate => candidate.key === key)
  if (!source || source.layer !== 'roads' || source.provenance !== 'national-proxy' ||
      dataset?.measurement !== 'proxy') {
    throw new Error(`${key} must be a registered national road proxy`)
  }
  return source.id
}

export const policySourceId = (policy: NationalRoadPolicy): number => nationalRoadProxySourceId(policy.country)

export async function enrichRoadsFromNationalPolicy(
  preparedDirectory: string,
  policy: NationalRoadPolicy,
) {
  const sourceId = policySourceId(policy)
  const squares = listPreparedSquares(preparedDirectory, policy.bbox)
  if (squares.length === 0) {
    throw new Error(`no ${policy.country} roads.arrow squares found under ${preparedDirectory}`)
  }
  const result = { country: policy.country, sourceId, rows: 0, matched: 0, retracted: 0,
    skipped: 0, skippedForeign: 0, squares: squares.length, squaresUpdated: 0 }
  for (const square of squares) {
    const write = await writeRoadAadt(resolve(preparedDirectory, square, 'roads.arrow'), row => {
      const traffic = policy.traffic(row)
      return traffic ? { ...traffic, sourceId } : null
    }, undefined, policy.coverage, {
      sourceIds: [sourceId],
      when: row => policy.traffic(row) === null,
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

function parseCli(argv: readonly string[]): { country: string; preparedDirectory: string } {
  const { values } = parseArgs({
    args: [...argv],
    strict: true,
    options: {
      country: { type: 'string' },
      'prepared-dir': { type: 'string' },
    },
  })
  if (!values.country || !values['prepared-dir']) {
    throw new Error('usage: enrich-roads-national-policy.ts --country CC --prepared-dir DIR')
  }
  return { country: values.country.toUpperCase(), preparedDirectory: resolve(values['prepared-dir']) }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const cli = parseCli(process.argv.slice(2))
  const policy = NATIONAL_ROAD_POLICIES.get(cli.country)
  if (!policy) {
    console.error(`no national road policy for ${cli.country}; expected one of ${[...NATIONAL_ROAD_POLICIES.keys()].join(',')}`)
    process.exitCode = 1
  } else {
    enrichRoadsFromNationalPolicy(cli.preparedDirectory, policy)
      .then(result => console.log(JSON.stringify(result)))
      .catch((error: unknown) => {
        console.error(error instanceof Error ? error.message : error)
        process.exitCode = 1
      })
  }
}
