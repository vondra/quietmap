/** Fill measured road-flow gaps inside each z9 owner, before the final taper pass. */

import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { parseArgs } from 'node:util'
import { withArrowWrite } from './lib/provenance.js'
import { applyRoadAadt } from './lib/roads-arrow.js'
import { listPreparedSquares } from './lib/prepared-grid.js'
import { readPlanningRoads, restorePreTaperFacts } from './lib/road-planning-input.js'
import { buildContinuityPlan, FILLABLE } from './lib/roads-continuity-plan.js'
import { SOURCE_ID_ROAD_CONTINUITY_HEURISTIC } from './lib/source-ids.generated.js'

export async function enrichContinuitySquare(path: string) {
  let counts = { rows: 0, matched: 0, retracted: 0, updated: false, anchors: 0, conflicts: 0 }
  await withArrowWrite(path, table => {
    const roads = readPlanningRoads(table)
    restorePreTaperFacts(roads)
    const { fill, anchors, conflicts } = buildContinuityPlan(roads)
    const applied = applyRoadAadt(table, path, (_row, index) => {
      const flow = fill.get(index)
      return flow ? { light: Math.round(flow.light), medium: Math.round(flow.medium),
        heavy: Math.round(flow.heavy), moto: Math.round(flow.moto), sourceId: SOURCE_ID_ROAD_CONTINUITY_HEURISTIC } : null
    }, undefined, FILLABLE,
    { sourceIds: [SOURCE_ID_ROAD_CONTINUITY_HEURISTIC], when: (_row, index) => !fill.has(index) })
    counts = { ...counts, ...applied.result, anchors, conflicts }
    return applied.table
  })
  return counts
}

async function main(): Promise<void> {
  const { values } = parseArgs({ options: { 'prepared-dir': { type: 'string' } } })
  if (!values['prepared-dir']) throw new Error('usage: enrich-roads-continuity-fill.ts --prepared-dir PREPARED_YEAR_DIR')
  const directory = resolve(values['prepared-dir']), squares = listPreparedSquares(directory, [-90, -180, 90, 180])
  if (!squares.length) throw new Error(`${directory}: no prepared road scope`)
  for (const square of squares) console.log(JSON.stringify({ square,
    ...await enrichContinuitySquare(resolve(directory, square, 'roads.arrow')) }))
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error: unknown) => { console.error(error); process.exitCode = 1 })
}
