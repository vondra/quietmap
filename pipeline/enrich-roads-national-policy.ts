/** Apply one registered country policy to baked-country z9 roads. */

import { roadObservation } from './lib/road-observation.js'
import { resolve } from 'node:path'
import { parseArgs } from 'node:util'
import { DATASETS } from './lib/enrichment-datasets.js'
import { NATIONAL_ROAD_POLICIES, type NationalRoadPolicy } from './lib/road-national-policies/index.js'
import { writeRoadAadt } from './lib/roads-arrow.js'
import { SOURCES_BY_KEY } from './lib/sources.js'
import { runRoadSquaresCli, writeNationalRoadSquares } from './lib/square-pool.js'

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
  const counters = await writeNationalRoadSquares(preparedDirectory, policy.bbox, policy.country, {}, path =>
    writeRoadAadt(path, row => {
      const traffic = policy.traffic(row)
      return traffic ? { ...traffic, sourceId, ...roadObservation({ policy: policy.country, traffic }, 'both-directions') } : null
    }, undefined, policy.coverage, {
      sourceIds: [sourceId],
      when: row => policy.traffic(row) === null,
    }))
  return { country: policy.country, sourceId, ...counters }
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

runRoadSquaresCli(import.meta.url, async () => {
  const cli = parseCli(process.argv.slice(2))
  const policy = NATIONAL_ROAD_POLICIES.get(cli.country)
  if (!policy) {
    throw new Error(`no national road policy for ${cli.country}; expected one of ${[...NATIONAL_ROAD_POLICIES.keys()].join(',')}`)
  }
  return enrichRoadsFromNationalPolicy(cli.preparedDirectory, policy)
})
