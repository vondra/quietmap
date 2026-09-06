/** Apply the complete dev1 no-open-timetable railway proxy family to z9 vectors. */

import { parseArgs } from 'node:util'
import { pathToFileURL } from 'node:url'
import { resolve } from 'node:path'
import { AFRICA_RAILWAY_PROXY_SPECS } from './lib/railway-proxy-africa.js'
import { EURASIA_RAILWAY_PROXY_SPECS } from './lib/railway-proxy-eurasia.js'
import type { RailwayProxySpec } from './lib/railway-proxy-rules.js'
import { listPreparedSquares } from './lib/prepared-grid.js'
import { writeRailwayTraffic } from './lib/railways-arrow.js'
import { inBbox } from './lib/spatial.js'
import { SOURCES_BY_ID, countryIsoForNationalSource } from './lib/sources.js'

export const RAILWAY_PROXY_SPECS: readonly RailwayProxySpec[] = [
  ...AFRICA_RAILWAY_PROXY_SPECS,
  ...EURASIA_RAILWAY_PROXY_SPECS,
].sort((left, right) => left.iso2.localeCompare(right.iso2))

export interface RailwayProxyResult {
  country: string
  status: 'enriched' | 'no-source'
  rows: number
  matched: number
  passengerTrainsPerDay: number
  freightTrainsPerDay: number
  skippedService: number
  skippedForeign: number
  skippedPriority: number
  squares: number
  squaresUpdated: number
}

interface RailwayProxyPlan {
  spec: RailwayProxySpec
  squares: readonly string[]
}

export function validateRailwayProxyCatalog(): void {
  const countries = new Set<string>()
  for (const spec of RAILWAY_PROXY_SPECS) {
    if (!/^[A-Z]{2}$/.test(spec.iso2) || countries.has(spec.iso2)) {
      throw new Error(`invalid or duplicate railway proxy country '${spec.iso2}'`)
    }
    countries.add(spec.iso2)
    const source = SOURCES_BY_ID.get(spec.sourceId)
    if (source?.layer !== 'railways' || source.provenance !== 'national-proxy' ||
        countryIsoForNationalSource(spec.sourceId) !== spec.iso2) {
      throw new Error(`railway proxy ${spec.iso2} has invalid source registration ${spec.sourceId}`)
    }
  }
  if (RAILWAY_PROXY_SPECS.length !== 17 ||
      RAILWAY_PROXY_SPECS.filter(spec => spec.classify === null).map(spec => spec.iso2).join() !== 'KR') {
    throw new Error('railway proxy catalog must contain 16 active countries plus KR no-source')
  }
}

function noSourceResult(country: string): RailwayProxyResult {
  return {
    country,
    status: 'no-source',
    rows: 0,
    matched: 0,
    passengerTrainsPerDay: 0,
    freightTrainsPerDay: 0,
    skippedService: 0,
    skippedForeign: 0,
    skippedPriority: 0,
    squares: 0,
    squaresUpdated: 0,
  }
}

function planCountry(preparedDirectory: string, spec: RailwayProxySpec): RailwayProxyPlan | null {
  if (spec.classify === null) return null
  const squares = listPreparedSquares(preparedDirectory, spec.bbox, 'railways.arrow')
  if (squares.length === 0) {
    throw new Error(`no ${spec.iso2} railways.arrow source squares found under ${preparedDirectory}`)
  }
  return { spec, squares }
}

async function runPlan(preparedDirectory: string, plan: RailwayProxyPlan): Promise<RailwayProxyResult> {
  const { spec, squares } = plan
  const classify = spec.classify!
  const result: RailwayProxyResult = {
    country: spec.iso2,
    status: 'enriched',
    rows: 0,
    matched: 0,
    passengerTrainsPerDay: 0,
    freightTrainsPerDay: 0,
    skippedService: 0,
    skippedForeign: 0,
    skippedPriority: 0,
    squares: squares.length,
    squaresUpdated: 0,
  }
  for (const square of squares) {
    const write = await writeRailwayTraffic(
      resolve(preparedDirectory, square, 'railways.arrow'),
      row => {
        if (!inBbox(row.midLat, row.midLon, spec.bbox)) return null
        const traffic = classify(row)
        return traffic ? { ...traffic, sourceId: spec.sourceId } : null
      },
      (_row, _index, applied) => {
        result.passengerTrainsPerDay += applied.passenger
        result.freightTrainsPerDay += applied.freight
      },
    )
    result.rows += write.rows
    result.matched += write.matched
    result.skippedService += write.skippedService
    result.skippedForeign += write.skippedForeign
    result.skippedPriority += write.skippedPriority
    if (write.updated) result.squaresUpdated++
  }
  return result
}

export async function enrichRailwayProxyCountry(
  preparedDirectory: string,
  iso2: string,
): Promise<RailwayProxyResult> {
  validateRailwayProxyCatalog()
  const country = iso2.toUpperCase()
  const spec = RAILWAY_PROXY_SPECS.find(candidate => candidate.iso2 === country)
  if (!spec) throw new Error(`unsupported railway proxy country '${iso2}'`)
  const plan = planCountry(resolve(preparedDirectory), spec)
  return plan ? runPlan(resolve(preparedDirectory), plan) : noSourceResult(country)
}

/** Preflight every active country's input before the first family write. */
export async function enrichAllRailwayProxies(
  preparedDirectory: string,
): Promise<readonly RailwayProxyResult[]> {
  validateRailwayProxyCatalog()
  const prepared = resolve(preparedDirectory)
  const plans = new Map<string, RailwayProxyPlan>()
  for (const spec of RAILWAY_PROXY_SPECS) {
    const plan = planCountry(prepared, spec)
    if (plan) plans.set(spec.iso2, plan)
  }
  const results: RailwayProxyResult[] = []
  for (const spec of RAILWAY_PROXY_SPECS) {
    const plan = plans.get(spec.iso2)
    results.push(plan ? await runPlan(prepared, plan) : noSourceResult(spec.iso2))
  }
  return results
}

function parsePreparedDirectory(argv: readonly string[]): string {
  const { values } = parseArgs({
    args: [...argv],
    strict: true,
    allowPositionals: false,
    options: { 'prepared-dir': { type: 'string' } },
  })
  if (!values['prepared-dir']) {
    throw new Error('usage: enrich-railway-proxies.ts --prepared-dir DIR')
  }
  return resolve(values['prepared-dir'])
}

async function main(): Promise<void> {
  const results = await enrichAllRailwayProxies(parsePreparedDirectory(process.argv.slice(2)))
  console.log(JSON.stringify(results))
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error: unknown) => {
    console.error(error instanceof Error ? error.message : error)
    process.exitCode = 1
  })
}
