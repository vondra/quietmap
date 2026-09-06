/** Apply final CZ-only transition ramps after every other road traffic writer. */

import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { parseArgs } from 'node:util'
import { withArrowWrite } from './lib/provenance.js'
import { applyRoadAadt } from './lib/roads-arrow.js'
import { bakedRoadCountryReader, iso2Code, listPreparedSquares } from './lib/prepared-grid.js'
import { readPlanningRoads, restorePreTaperFacts } from './lib/road-planning-input.js'
import { buildTaperPlan, TAPER_CLASSES } from './lib/roads-taper-plan.js'
import { CZ_SPEEDS } from './lib/road-planning-defaults.generated.js'
import { SOURCE_ID_OSM_TRANSITION_TAPER } from './lib/source-ids.generated.js'

export async function enrichTaperSquare(path: string) {
  let counts = { rows: 0, matched: 0, retracted: 0, updated: false, boundaries: 0, skippedUnscaled: 0, foreignRows: 0 }
  await withArrowWrite(path, table => {
    const countries = bakedRoadCountryReader(table), roads = readPlanningRoads(table)
    const cz = roads.filter(road => countries.codeAt(road.i) === iso2Code('CZ'))
    // Reconstruct authored facts before planning; prior ramps are never anchors.
    restorePreTaperFacts(cz)
    const { plan, stats } = buildTaperPlan(cz, CZ_SPEEDS)
    const applied = applyRoadAadt(table, path, (_row, index) => {
      const entry = plan.get(index)
      if (!entry) return null
      const [light, medium, heavy, moto] = entry.aadt ?? [0, 0, 0, 0]
      return { light, medium, heavy, moto, sourceId: SOURCE_ID_OSM_TRANSITION_TAPER, speedTaper: entry.speed }
    }, undefined, TAPER_CLASSES,
    { sourceIds: [SOURCE_ID_OSM_TRANSITION_TAPER], when: (_row, index) => !plan.has(index) })
    counts = { ...counts, ...applied.result, boundaries: stats.boundaries,
      skippedUnscaled: stats.skippedUnscaled, foreignRows: roads.length - cz.length }
    return applied.table
  })
  return counts
}

async function main(): Promise<void> {
  const { values } = parseArgs({ options: { 'prepared-dir': { type: 'string' } } })
  if (!values['prepared-dir']) throw new Error('usage: enrich-roads-taper.ts --prepared-dir PREPARED_YEAR_DIR')
  const directory = resolve(values['prepared-dir']), squares = listPreparedSquares(directory, [-90, -180, 90, 180])
  if (!squares.length) throw new Error(`${directory}: no prepared road scope`)
  for (const square of squares) console.log(JSON.stringify({ square,
    ...await enrichTaperSquare(resolve(directory, square, 'roads.arrow')) }))
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error: unknown) => { console.error(error); process.exitCode = 1 })
}
