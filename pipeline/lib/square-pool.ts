/** Shard per-square work across QM_ROAD_WORKERS child processes: world heuristics by `--shard`, national road loaders by re-spawned command line. */

import { spawn } from 'node:child_process'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { parseArgs } from 'node:util'
import { listPreparedSquares, type PreparedBbox } from './prepared-grid.js'
import type { WriteRoadResult } from './roads-arrow.js'
import { availableMemoryBytes, cpuJobs, fitJobs } from './worker-jobs.js'

/** One Node square-heuristic process; a 20 GiB layer cgroup holds 20. */
export const SQUARE_WORKER_BYTES = 1 << 30

export function workerCount(
  env: NodeJS.ProcessEnv = process.env, perWorkerBytes = SQUARE_WORKER_BYTES, memoryBytes = availableMemoryBytes(),
): number {
  const raw = env.QM_ROAD_WORKERS
  const requested = raw === undefined || raw === '' ? cpuJobs() : Number(raw)
  if (!Number.isInteger(requested) || requested < 1) throw new Error('QM_ROAD_WORKERS must be a positive integer')
  return fitJobs(requested, perWorkerBytes, memoryBytes)
}

export function parseShard(shard: string | undefined): { index: number; count: number } {
  if (shard === undefined) return { index: 0, count: 1 }
  const [index, count] = shard.split('/').map(Number)
  if (!Number.isInteger(index) || !Number.isInteger(count) || count < 1 || index < 0 || index >= count) {
    throw new Error(`invalid --shard ${shard}; expected index/count`)
  }
  return { index, count }
}

/** Squares are independent files, so shard `index/count` owns every count-th square. */
export function shardSquares(squares: string[], shard: { index: number; count: number }): string[] {
  return squares.filter((_, position) => position % shard.count === shard.index)
}

export function argvHasShard(argv: readonly string[] = process.argv): boolean {
  return argv.some(arg => arg === '--shard' || arg.startsWith('--shard='))
}

/**
 * Re-spawn this process once per shard when no `--shard` is set and more than one
 * worker fits. Forwards the original argv so extra flags (`--world`, `--enrichment-dir`)
 * reach the children. Returns true in the parent after the children exit.
 */
export async function fanOutIfNeeded(perWorkerBytes = SQUARE_WORKER_BYTES): Promise<boolean> {
  if (argvHasShard()) return false
  const workers = workerCount(process.env, perWorkerBytes)
  if (workers <= 1) return false
  await Promise.all(Array.from({ length: workers }, (_, index) => new Promise<void>((done, fail) => {
    const child = spawn(process.execPath, [
      ...process.execArgv, process.argv[1], ...process.argv.slice(2), '--shard', `${index}/${workers}`,
    ], { stdio: 'inherit', env: { ...process.env, QM_ROAD_WORKERS: '1' } })
    child.on('error', fail)
    child.on('exit', code => (code === 0 ? done() : fail(new Error(`shard ${index}/${workers} exited ${code}`))))
  })))
  return true
}

/**
 * Entry point shared by the per-square heuristics: `--prepared-dir DIR [--shard i/n]`.
 * Without `--shard`, enough workers re-spawn this same script once per shard
 * and inherit their stdout, so the chain log keeps one line per square.
 */
export async function runSquareSteps(
  usage: string, step: (squareDirectory: string) => Promise<object>, bbox: PreparedBbox = [-90, -180, 90, 180],
): Promise<void> {
  const { values } = parseArgs({ options: { 'prepared-dir': { type: 'string' }, shard: { type: 'string' } } })
  if (!values['prepared-dir']) throw new Error(usage)
  const directory = resolve(values['prepared-dir'])
  if (await fanOutIfNeeded()) return
  const squares = listPreparedSquares(directory, bbox)
  if (!squares.length) throw new Error(`${directory}: no prepared road scope`)
  for (const square of shardSquares(squares, parseShard(values.shard))) {
    console.log(JSON.stringify({ square, ...(await step(resolve(directory, square))) }))
  }
}

const OWN_SQUARE_SHARD_ENV = 'QM_ROAD_SQUARE_SHARD'

/** Set only in a child of `writeRoadSquaresAcrossShards`: the share of every square walk this process owns. */
export const ownSquareShard = process.env[OWN_SQUARE_SHARD_ENV] === undefined
  ? null : parseShard(process.env[OWN_SQUARE_SHARD_ENV])

const squareWriteCounters = () => ({ rows: 0, matched: 0, retracted: 0, skipped: 0, skippedForeign: 0,
  squares: 0, squaresUpdated: 0 })
export type SquareWriteCounters = ReturnType<typeof squareWriteCounters>
type ShardWalkTotals = Record<string, number>

let fanOutFromThisCli = false
let squareWalksStarted = 0
let shardWalkTotals: Promise<ShardWalkTotals[][] | null> | undefined

/**
 * Run a road loader when its module is the process entry point and print its receipt.
 * Only such a process may fan out: a test or importer calling a loader function walks serially.
 */
export function runRoadSquaresCli(moduleUrl: string, main: () => Promise<object>): void {
  if (!process.argv[1] || moduleUrl !== pathToFileURL(resolve(process.argv[1])).href) return
  fanOutFromThisCli = true
  main().then(receipt => { if (!ownSquareShard) console.log(JSON.stringify(receipt)) })
    .catch((error: unknown) => {
      console.error(error instanceof Error ? error.message : error)
      process.exitCode = 1
    })
}

/** Each shard repeats the parent's source load, so one worker costs what the parent holds now plus one decoded square. */
function spawnSquareShards(squareCount: number): Promise<ShardWalkTotals[][] | null> {
  const loadedBytes = process.memoryUsage().rss
  const workers = Math.min(squareCount, workerCount(process.env, loadedBytes + SQUARE_WORKER_BYTES,
    Math.max(1, availableMemoryBytes() - loadedBytes)))
  if (workers <= 1) return Promise.resolve(null)
  return Promise.all(Array.from({ length: workers }, (_, index) => new Promise<ShardWalkTotals[]>((done, fail) => {
    const walks: ShardWalkTotals[] = []
    const child = spawn(process.execPath, [...process.execArgv, ...process.argv.slice(1)], {
      stdio: ['ignore', 'inherit', 'inherit', 'ipc'],
      env: { ...process.env, QM_ROAD_WORKERS: '1', [OWN_SQUARE_SHARD_ENV]: `${index}/${workers}` },
    })
    child.on('message', totals => walks.push(totals as ShardWalkTotals))
    child.on('error', fail)
    child.on('close', code => (code === 0 ? done(walks) : fail(new Error(`shard ${index}/${workers} exited ${code}`))))
  })))
}

/**
 * Write every square once and sum the write counters plus the caller's `tally`.
 * A CLI parent re-spawns its own command line once per shard at its first walk; each child
 * repeats every walk over its share and reports totals per walk in call order. `squares`
 * must therefore be the same list in every process: derive it from the directory tree and
 * the source only, never from file contents a sibling shard is rewriting.
 */
export async function writeRoadSquaresAcrossShards(
  preparedDirectory: string,
  squares: readonly string[],
  tally: Record<string, number>,
  writeSquare: (roadsArrowPath: string) => Promise<WriteRoadResult>,
): Promise<SquareWriteCounters> {
  const walk = squareWalksStarted++
  const counters = squareWriteCounters()
  if (fanOutFromThisCli && !ownSquareShard) {
    const shards = await (shardWalkTotals ??= spawnSquareShards(squares.length))
    if (shards) {
      for (const [index, walks] of shards.entries()) {
        const totals = walks[walk]
        if (!totals) throw new Error(`shard ${index}/${shards.length} reported no totals for square walk ${walk}`)
        for (const key of Object.keys(counters) as (keyof SquareWriteCounters)[]) counters[key] += totals[key]
        for (const key of Object.keys(tally)) tally[key] += totals[key]
      }
      return counters
    }
  }
  const owned = ownSquareShard ? shardSquares([...squares], ownSquareShard) : squares
  counters.squares = owned.length
  for (const square of owned) {
    const write = await writeSquare(resolve(preparedDirectory, square, 'roads.arrow'))
    counters.rows += write.rows
    counters.matched += write.matched
    counters.retracted += write.retracted
    counters.skipped += write.skipped
    counters.skippedForeign += write.skippedForeign
    if (write.updated) counters.squaresUpdated++
  }
  if (ownSquareShard) {
    await new Promise<void>((done, fail) =>
      process.send!({ ...counters, ...tally }, (error: Error | null) => (error ? fail(error) : done())))
  }
  return counters
}

/** The national loaders' walk: every prepared square in the source's bounding box. */
export async function writeNationalRoadSquares(
  preparedDirectory: string,
  bbox: PreparedBbox,
  country: string,
  tally: Record<string, number>,
  writeSquare: (roadsArrowPath: string) => Promise<WriteRoadResult>,
): Promise<SquareWriteCounters> {
  const squares = listPreparedSquares(preparedDirectory, bbox)
  if (squares.length === 0) throw new Error(`no ${country} roads.arrow squares found under ${preparedDirectory}`)
  return writeRoadSquaresAcrossShards(preparedDirectory, squares, tally, writeSquare)
}
