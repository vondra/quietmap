/** Fit the carriageway priors of classes 0-4 on counted rows of training squares of a finalized year, score them on holdout squares, and regenerate the engine table. */

import { readFileSync, writeFileSync } from 'node:fs'
import { fork } from 'node:child_process'
import { availableParallelism } from 'node:os'
import { resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { parseArgs } from 'node:util'
import { tableFromIPC } from 'apache-arrow'
import { isCountHoldoutSquare } from './lib/count-holdout.js'
import { DATASETS } from './lib/enrichment-datasets.js'
import { listPreparedSquares } from './lib/prepared-grid.js'
import { isMeasured } from './lib/sources.js'

const GENERATED = new URL('../engine/noise-compute/src/road_traffic_priors_generated.rs', import.meta.url)
const PRIOR_CLASSES = 5
/** Classes with a per-lane rate; secondary and tertiary lanes tags are too sparse to carry one. */
const PER_LANE_CLASSES = 3
const FIELDS = 7 // class, one-way, built-up, lanes, count, length, holdout
const COUNTED_SOURCES = new Set(DATASETS.filter(dataset => dataset.measurement === 'counted' && isMeasured(dataset.id))
  .map(dataset => dataset.id))

/** Counted public carriageways of classes 0-4 outside tunnels and roundabouts, one packed record per row.
 *  Rows with no observed class (`traffic_estimated` 15: DfT scaled rows, allocated splits) are not truth:
 *  they train nothing and score nothing. */
export function squareSamples(preparedDirectory: string, square: string): Float64Array {
  const table = tableFromIPC(readFileSync(resolve(preparedDirectory, square, 'roads.arrow')))
  if (table.schema.metadata.get('road_traffic_contract') !== '1') throw new Error(`${square}: roads.arrow is not finalized`)
  const column = (name: string) => table.getChild(name)!.toArray() as ArrayLike<number>
  const [classes, oneway, junction, lanes, builtUp, access, sources, lengths, estimated] =
    ['road_class', 'oneway', 'junction', 'lanes', 'built_up', 'access', 'source_id', 'length_m', 'traffic_estimated'].map(column)
  const tunnel = table.getChild('tunnel')!
  const counts = ['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto'].map(column)
  const [, x, y] = square.split('/').map(Number)
  const holdout = isCountHoldoutSquare(x, y) ? 1 : 0
  const samples: number[] = []
  for (let row = 0; row < table.numRows; row++) {
    const count = counts[0][row] + counts[1][row] + counts[2][row] + counts[3][row]
    if (classes[row] >= PRIOR_CLASSES || !COUNTED_SOURCES.has(sources[row]) || junction[row] !== 0 || access[row] !== 0 ||
        estimated[row] === 15 || tunnel.get(row) || !(count > 0) || !(lengths[row] > 0)) continue
    samples.push(classes[row], oneway[row] === 0 ? 0 : 1, builtUp[row], lanes[row], count, lengths[row], holdout)
  }
  return Float64Array.from(samples)
}

export interface Sample { roadClass: number; oneWay: boolean; builtUp: number; lanes: number; count: number; length: number; holdout: boolean }

function unpack(packed: Float64Array): Sample[] {
  return Array.from({ length: packed.length / FIELDS }, (_, row) => {
    const at = (field: number) => packed[row * FIELDS + field]
    return { roadClass: at(0), oneWay: at(1) === 1, builtUp: at(2), lanes: at(3), count: at(4), length: at(5), holdout: at(6) === 1 }
  })
}

function weightedMedian(values: ReadonlyArray<readonly [value: number, weight: number]>): number {
  const sorted = [...values].sort((a, b) => a[0] - b[0])
  const half = sorted.reduce((sum, [, weight]) => sum + weight, 0) / 2
  let cumulative = 0
  for (const [value, weight] of sorted) if ((cumulative += weight) >= half) return value
  return NaN
}

const tagged = (lanes: number) => lanes >= 1 && lanes <= 6
export interface Prior { vehiclesPerLane: number; untagged: number; km: number }
/** `[class][one-way, two-way][built-up unknown = every row, rural, urban]`, as in the engine. */
export type PriorTable = Prior[][][]

/** Holdout rule v1: counts of holdout squares never train a prior. */
export function fitPriors(samples: readonly Sample[]): PriorTable {
  const training = samples.filter(sample => !sample.holdout)
  return Array.from({ length: PRIOR_CLASSES }, (_, roadClass) => [true, false].map(oneWay => [0, 1, 2].map(builtUp => {
    // A one-way secondary street shares the fitted two-way section instead of a
    // one-way arm (w3-priors, 2026-09-25): these cells stay zero, `predict` halves.
    if (roadClass === 3 && oneWay) return { vehiclesPerLane: 0, untagged: 0, km: 0 }
    const cell = training.filter(sample => sample.roadClass === roadClass && sample.oneWay === oneWay &&
      (builtUp === 0 || sample.builtUp === builtUp))
    const perLane = roadClass < PER_LANE_CLASSES
    const rate = weightedMedian(cell.filter(sample => perLane && tagged(sample.lanes)).map(s => [s.count / s.lanes, s.length]))
    const whole = weightedMedian(cell.filter(sample => !perLane || !tagged(sample.lanes)).map(s => [s.count, s.length]))
    if (!Number.isFinite(whole) || (perLane && !Number.isFinite(rate))) throw new Error(`no counted rows for class ${roadClass}`)
    return { vehiclesPerLane: perLane ? Math.round(rate) : 0, untagged: Math.round(whole),
      km: Math.round(cell.reduce((sum, sample) => sum + sample.length, 0) / 1000) }
  })))
}

export function predict(table: PriorTable, sample: Sample): number {
  if (sample.roadClass === 3 && sample.oneWay) {
    return predict(table, { ...sample, oneWay: false }) / 2
  }
  const prior = table[sample.roadClass][sample.oneWay ? 0 : 1][Math.min(sample.builtUp, 2)]
  return prior.vehiclesPerLane > 0 && tagged(sample.lanes) ? sample.lanes * prior.vehiclesPerLane : prior.untagged
}

/** Length-weighted 10 lg(prior/count) on held-out rows, per class, direction and place. */
export function scoreOnHoldout(table: PriorTable, samples: readonly Sample[]) {
  const score = (cell: readonly Sample[]) => {
    const km = cell.reduce((sum, sample) => sum + sample.length, 0)
    const errors = cell.map(sample => [10 * Math.log10(predict(table, sample) / sample.count), sample.length] as const)
    return { km: Math.round(km / 1000),
      biasDb: Number((errors.reduce((sum, [error, length]) => sum + error * length, 0) / km).toFixed(2)),
      maeDb: Number((errors.reduce((sum, [error, length]) => sum + Math.abs(error) * length, 0) / km).toFixed(2)),
      medianDb: Number(weightedMedian(errors).toFixed(2)) }
  }
  const held = samples.filter(sample => sample.holdout)
  return Object.fromEntries(Array.from({ length: PRIOR_CLASSES }, (_, roadClass) => [true, false].flatMap(oneWay => [1, 2].map(builtUp => {
    const cell = held.filter(sample => sample.roadClass === roadClass && sample.oneWay === oneWay && sample.builtUp === builtUp)
    return [`class ${roadClass} ${oneWay ? 'one-way' : 'two-way'} ${builtUp === 1 ? 'rural' : 'urban'}`, cell.length ? score(cell) : null]
  }))).flat())
}

export function priorTableRust(table: PriorTable, provenance: string): string {
  const cell = (prior: Prior) => `p(${prior.vehiclesPerLane.toFixed(1)}, ${prior.untagged.toFixed(1)})`
  const rows = table.map((byDirection, roadClass) =>
    `    // class ${roadClass}, km per cell: ${byDirection.map(cells => cells.map(prior => prior.km).join('/')).join(' | ')}\n` +
    `    [${byDirection.map(cells => `[${cells.map(cell).join(', ')}]`).join(', ')}],`).join('\n')
  return `//! Measured carriageway priors for classes 0-4. Generated by pipeline/fit-road-traffic-priors.ts; do not edit.
//!
//! ${provenance}
//! Length-weighted medians of counted public carriageways (tunnels, roundabouts, derived flows and fully estimated rows excluded)
//! in training squares of holdout rule v1: vehicles per lane for a lanes tag of 1-6 (classes 0-2), else
//! the whole carriageway count. Class-3 one-way cells stay zero: one-way secondary streets share the
//! fitted two-way section instead (w3-priors, 2026-09-25).

use crate::defaults::CarriagewayPrior;

const fn p(vehicles_per_lane: f64, untagged: f64) -> CarriagewayPrior {
    CarriagewayPrior { vehicles_per_lane, untagged }
}

/// \`[class][one-way, two-way][built-up unknown (every row), rural, urban]\`.
pub const MEASURED_CARRIAGEWAY_PRIORS: [[[CarriagewayPrior; 3]; 2]; 5] = [
${rows}
];
`
}

async function collectSamples(preparedDirectory: string, workers: number): Promise<Sample[]> {
  const squares = listPreparedSquares(preparedDirectory, [-90, -180, 90, 180])
  const packed: Float64Array[] = []
  const pool = Array.from({ length: Math.min(workers, squares.length) }, () =>
    fork(fileURLToPath(import.meta.url), ['--worker', preparedDirectory], { serialization: 'advanced' }))
  let next = 0
  await Promise.all(pool.map(worker => new Promise<void>((done, fail) => {
    const give = () => next < squares.length ? worker.send(squares[next++]) : (worker.kill(), done())
    worker.on('message', (reply: Float64Array | { error: string }) => {
      if (!(reply instanceof Float64Array)) return fail(new Error(reply.error))
      packed.push(reply)
      give()
    })
    worker.on('exit', code => { if (code && next <= squares.length) fail(new Error(`prior fit worker exited ${code}`)) })
    give()
  })))
  return packed.flatMap(unpack)
}

async function main(): Promise<void> {
  const { values } = parseArgs({ options: {
    'prepared-dir': { type: 'string' }, release: { type: 'string' }, write: { type: 'boolean', default: false },
    jobs: { type: 'string' }, worker: { type: 'boolean', default: false },
  }, allowPositionals: true })
  if (!values['prepared-dir'] || !values.release) {
    throw new Error('usage: fit-road-traffic-priors.ts --prepared-dir FINALIZED_YEAR_DIR --release NAME [--jobs N] [--write]')
  }
  const samples = await collectSamples(resolve(values['prepared-dir']), Number(values.jobs ?? availableParallelism()))
  const fitted = fitPriors(samples)
  const report = { release: values.release, trainingKm: Math.round(samples.filter(s => !s.holdout).reduce((sum, s) => sum + s.length, 0) / 1000),
    holdoutKm: Math.round(samples.filter(s => s.holdout).reduce((sum, s) => sum + s.length, 0) / 1000),
    fitted, holdout: scoreOnHoldout(fitted, samples) }
  console.log(JSON.stringify(report, null, 1))
  if (values.write) {
    writeFileSync(GENERATED, priorTableRust(fitted, `Fitted on release ${values.release}, ${new Date().toISOString().slice(0, 10)}: ` +
      `${report.trainingKm} km of training rows; scored on ${report.holdoutKm} km of holdout rows.`))
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  if (process.argv.includes('--worker')) {
    const prepared = process.argv[process.argv.indexOf('--worker') + 1]
    process.on('message', (square: string) => {
      try { process.send!(squareSamples(prepared, square)) } catch (error) { process.send!({ error: String(error) }) }
    })
  } else {
    main().catch((error: unknown) => { console.error(error); process.exitCode = 1 })
  }
}
