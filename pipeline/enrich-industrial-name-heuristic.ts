/** Complete industrial names after registry classification on the selected native prepared tree. */

import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { parseArgs } from 'node:util'
import { enrichIndustrialNames } from './lib/industrial-name.js'

async function main(): Promise<void> {
  const { values } = parseArgs({ options: { 'prepared-dir': { type: 'string' } } })
  if (!values['prepared-dir']) throw new Error('usage: enrich-industrial-name-heuristic.ts --prepared-dir PREPARED_YEAR_DIR')
  console.log(JSON.stringify(await enrichIndustrialNames(resolve(values['prepared-dir']))))
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error: unknown) => { console.error(error); process.exitCode = 1 })
}
