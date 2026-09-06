/** Apply the three admitted municipal traffic sources inside their true ADM2 boundaries. */

import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { parseArgs } from 'node:util'
import { loadMunicipalRoadSources, type CityRoadRecord } from './lib/city-roads-source.js'
import { municipalRoadMatcher } from './lib/city-roads-match.js'
import { applyRoadAadt, writeRoadAadt, type RoadRow } from './lib/roads-arrow.js'
import { bakedRoadCountryReader, iso2Code, listPreparedSquares } from './lib/prepared-grid.js'
import { withArrowWrite } from './lib/provenance.js'

type AdmittedCity = Awaited<ReturnType<typeof loadMunicipalRoadSources>>['cities'][number]

export async function enrichMunicipalRoads(preparedDirectory: string, cities: readonly AdmittedCity[]) {
  const plans = cities.map(city => ({ city, matcher: municipalRoadMatcher(city.records, city.coverage),
    hasGeometry: city.records.some(record => record.line !== undefined), expectedCountryCode: iso2Code(city.country.toUpperCase()),
    paths: listPreparedSquares(preparedDirectory, city.municipality.bbox).map(square => resolve(preparedDirectory, square, 'roads.arrow')) }))
  // Admit every selected Arrow and observe real street geometry before any city can replace a file.
  for (const plan of plans) {
    if (!plan.paths.length) throw new Error(`${plan.city.slug}: no prepared road units in municipal scope`)
    for (const path of plan.paths) await withArrowWrite(path, table => {
      const countries = bakedRoadCountryReader(table)
      if (!plan.hasGeometry) {
        applyRoadAadt(table, path, () => null)
        return table
      }
      applyRoadAadt(table, path, (row, index) => {
        if (countries.codeAt(index) === plan.expectedCountryCode && plan.city.municipality.contains(row.midLat, row.midLon)) plan.matcher.observe(row)
        return null
      })
      return table
    })
  }
  const runs = []
  for (const plan of plans) {
    const unmatchableGeometry = plan.matcher.finish()
    const recordFor = (row: RoadRow): CityRoadRecord | null => plan.city.municipality.contains(row.midLat, row.midLon) ? plan.matcher.match(row) : null
    const toAadt = (record: CityRoadRecord | null) => record ? { light: record.light, medium: record.medium, heavy: record.heavy, moto: record.moto, sourceId: plan.city.sourceId } : null
    const candidateFor = (row: RoadRow) => toAadt(recordFor(row))
    const retract = { sourceIds: [plan.city.sourceId], when: (row: RoadRow) => !plan.city.coverage.has(row.roadClass) || recordFor(row) === null }
    let applicable = 0
    for (const path of plan.paths) await withArrowWrite(path, table => {
      const countries = bakedRoadCountryReader(table)
      applyRoadAadt(table, path, (row, index) => {
        const record = recordFor(row)
        if (record && countries.codeAt(index) === plan.expectedCountryCode) applicable++
        return toAadt(record)
      }, undefined, plan.city.coverage, retract)
      return table
    })
    if (applicable === 0) throw new Error(`${plan.city.slug}: no rows matched admitted municipal traffic`)
    runs.push({ ...plan, unmatchableGeometry, candidateFor, retract })
  }
  const results = []
  for (const run of runs) {
    let matched = 0, updated = 0, retracted = 0
    for (const path of run.paths) {
      const result = await writeRoadAadt(path, run.candidateFor, undefined, run.city.coverage, run.retract)
      matched += result.matched; updated += Number(result.updated); retracted += result.retracted
    }
    results.push({ city: run.city.slug, year: run.city.year, admission: run.city.admission, records: run.city.records.length, zeroSplitSkipped: run.city.zeroSplitSkipped,
      squares: run.paths.length, matched, updated, retracted, unmatchableGeometry: run.unmatchableGeometry })
  }
  return results
}

async function main(): Promise<void> {
  const { values } = parseArgs({ options: {
    'prepared-dir': { type: 'string' }, 'enrichment-dir': { type: 'string' }, 'boundaries-dir': { type: 'string' },
  } })
  if (!values['prepared-dir'] || !values['enrichment-dir'] || !values['boundaries-dir']) throw new Error('usage: enrich-cities-roads.ts --prepared-dir PREPARED_YEAR_DIR --enrichment-dir ENRICHMENT_YEAR_DIR --boundaries-dir ADM2_DIRECTORY')
  const source = await loadMunicipalRoadSources(resolve(values['enrichment-dir']), resolve(values['boundaries-dir']))
  console.log(JSON.stringify({ sources: source.receipts }))
  console.log(JSON.stringify(await enrichMunicipalRoads(resolve(values['prepared-dir']), source.cities)))
}
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error: unknown) => { console.error(error); process.exitCode = 1 })
}
