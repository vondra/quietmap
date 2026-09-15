/** Apply CZ speed transitions without changing traffic or its provenance. */

import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { DataType, Table, makeVector } from 'apache-arrow'
import { withArrowWrite } from './lib/provenance.js'
import { bakedRoadCountryReader, iso2Code } from './lib/prepared-grid.js'
import { readPlanningRoads } from './lib/road-planning-input.js'
import { buildTaperPlan } from './lib/roads-taper-plan.js'
import { CZ_SPEEDS } from './lib/road-planning-defaults.generated.js'
import { runSquareSteps } from './lib/square-pool.js'

export async function enrichTaperSquare(path: string) {
  const counts = { rows: 0, matched: 0, retracted: 0, updated: false, boundaries: 0, foreignRows: 0 }
  await withArrowWrite(path, table => {
    const rows = table.numRows
    const countries = bakedRoadCountryReader(table), czechCode = iso2Code('CZ')
    const existing = table.getChild('speed_taper')
    if (existing && (!DataType.isInt(existing.type) || existing.type.bitWidth !== 8 || existing.type.isSigned || existing.nullCount)) {
      throw new Error(`${path}: invalid speed_taper column`)
    }
    let hasCzechRoads = false
    for (let index = 0; index < rows; index++) {
      if (countries.codeAt(index) === czechCode) { hasCzechRoads = true; break }
    }
    if (!hasCzechRoads) {
      counts.rows = counts.foreignRows = rows
      return table
    }
    const roads = readPlanningRoads(table)
    const cz = roads.filter(road => countries.codeAt(road.i) === czechCode)
    const { plan, stats } = buildTaperPlan(cz, CZ_SPEEDS)
    const speed = Uint8Array.from(roads, road => Number(existing?.get(road.i) ?? 0))
    for (const road of cz) {
      const next = plan.get(road.i)?.speed ?? 0
      if (speed[road.i] !== next) {
        if (speed[road.i] > 0 && next === 0) counts.retracted++
        speed[road.i] = next
        counts.updated = true
      }
    }
    Object.assign(counts, { rows: roads.length, matched: plan.size,
      boundaries: stats.boundaries, foreignRows: roads.length - cz.length })
    if (!counts.updated) return table
    return new Table({
      ...Object.fromEntries(table.schema.fields.map(field => [field.name, table.getChild(field.name)!])),
      speed_taper: makeVector(speed),
    })
  })
  return counts
}

async function main(): Promise<void> {
  await runSquareSteps('usage: enrich-roads-taper.ts --prepared-dir PREPARED_YEAR_DIR', directory => enrichTaperSquare(resolve(directory, 'roads.arrow')))
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error: unknown) => { console.error(error); process.exitCode = 1 })
}
