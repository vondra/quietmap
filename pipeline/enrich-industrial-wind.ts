/** Apply the complete retained seven-country turbine parameter family to native industrial rows. */

import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { parseArgs } from 'node:util'
import { loadWindRegisters } from './lib/industrial-wind-source.js'
import { windParameterMatcher, enrichWindSquare } from './lib/industrial-wind-arrow.js'
import { listPreparedSquares } from './lib/prepared-grid.js'

async function main(): Promise<void> {
  const { values } = parseArgs({ options: { 'prepared-dir': { type: 'string' }, 'enrichment-dir': { type: 'string' } } })
  if (!values['prepared-dir'] || !values['enrichment-dir']) throw new Error('usage: enrich-industrial-wind.ts --prepared-dir PREPARED_YEAR_DIR --enrichment-dir ENRICHMENT_YEAR_DIR')
  const source = await loadWindRegisters(resolve(values['enrichment-dir']))
  console.log(JSON.stringify({ sources: source.receipts, admitted: source.registers.map(r => ({ country: r.country, observations: r.observations.length })) }))
  const directory = resolve(values['prepared-dir']), squares = listPreparedSquares(directory, [-90, -180, 90, 180], 'industrial.arrow')
  if (!squares.length) throw new Error(`${directory}: no industrial Arrow scope`)
  const match = windParameterMatcher(source.registers)
  for (const square of squares) console.log(JSON.stringify({ square, ...await enrichWindSquare(resolve(directory, square, 'industrial.arrow'), match) }))
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error: unknown) => { console.error(error); process.exitCode = 1 })
}
