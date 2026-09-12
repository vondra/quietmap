/** Complete industrial names after registry classification on the selected native prepared tree. */

import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { parseArgs } from 'node:util'
import { enrichIndustrialNames } from './lib/industrial-name.js'
import { listPreparedSquares } from './lib/prepared-grid.js'
import { fanOutIfNeeded, parseShard, shardSquares } from './lib/square-pool.js'

async function main(): Promise<void> {
  const { values } = parseArgs({ options: { 'prepared-dir': { type: 'string' }, shard: { type: 'string' } } })
  if (!values['prepared-dir']) throw new Error('usage: enrich-industrial-name-heuristic.ts --prepared-dir PREPARED_YEAR_DIR')
  if (await fanOutIfNeeded()) return
  const directory = resolve(values['prepared-dir'])
  console.log(JSON.stringify(await enrichIndustrialNames(
    directory,
    shardSquares(listPreparedSquares(directory, [-90, -180, 90, 180], 'industrial.arrow'), parseShard(values.shard)),
  )))
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error: unknown) => { console.error(error); process.exitCode = 1 })
}
