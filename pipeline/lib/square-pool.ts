/** Shard per-square work across QM_ROAD_WORKERS child processes: world heuristics by `--shard`, national road loaders by re-spawned command line. */

import { spawn, type ChildProcess } from 'node:child_process'
import { statSync } from 'node:fs'
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

/**
 * A square's owner depends on the square alone, never on its position in one list: a loader that
 * walks two lists (DE: Germany, then Baden-Württemberg) keeps each file in one sequential process.
 * Measured on r260919 at 20 shards, max/mean bytes per shard: world 1.039, US 1.142, DE 1.111.
 */
export function shardSquares(squares: readonly string[], shard: { index: number; count: number }): string[] {
  return squares.filter(square => {
    const [, x, y] = square.split('/').map(Number)
    return (x * 512 + y) % shard.count === shard.index
  })
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
const SAMPLED_SQUARE_ENV = 'QM_ROAD_SQUARE_SAMPLED'

/** Set only in a child of `writeRoadSquaresAcrossShards`: the share of the square walk this process owns. */
export const ownSquareShard = process.env[OWN_SQUARE_SHARD_ENV] === undefined
  ? null : parseShard(process.env[OWN_SQUARE_SHARD_ENV])

const squareWriteCounters = () => ({ rows: 0, matched: 0, retracted: 0, skipped: 0, skippedForeign: 0,
  squares: 0, squaresUpdated: 0 })
export type SquareWriteCounters = ReturnType<typeof squareWriteCounters> & { shards: number }
interface ShardTotals { counters: ReturnType<typeof squareWriteCounters>; tally: Record<string, number> }

let fanOutFromThisCli = false
let shardedWalkStarted = false

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
    // The parent waits for 'close'; Node already unrefs a listener-less channel, this makes the end explicit.
    .finally(() => { if (ownSquareShard) process.disconnect?.() })
}

/**
 * A shard repeats the parent's whole start and source load. It therefore pays only while its share of
 * the walk lasts at least as long as that load, and it needs the parent's peak memory plus one decoded square.
 */
function squareShardCount(squares: number, loadSeconds: number, walkSeconds: number): number {
  const peakBytes = process.resourceUsage().maxRSS * 1024
  return Math.min(squares, Math.ceil(walkSeconds / loadSeconds), workerCount(process.env,
    peakBytes + SQUARE_WORKER_BYTES, Math.max(1, availableMemoryBytes(process.memoryUsage().rss))))
}

function spawnSquareShards(shards: number, sampledSquare: string): Promise<ShardTotals[]> {
  const children: ChildProcess[] = []
  return Promise.all(Array.from({ length: shards }, (_, index) => new Promise<ShardTotals>((done, fail) => {
    const reported: ShardTotals[] = []
    const child = spawn(process.execPath, [...process.execArgv, ...process.argv.slice(1)], {
      stdio: ['ignore', 'inherit', 'inherit', 'ipc'],
      env: { ...process.env, QM_ROAD_WORKERS: '1', [OWN_SQUARE_SHARD_ENV]: `${index}/${shards}`,
        [SAMPLED_SQUARE_ENV]: sampledSquare },
    })
    children.push(child)
    child.on('message', totals => reported.push(totals as ShardTotals))
    child.on('error', fail)
    child.on('close', (code, signal) => (code === 0 && reported.length === 1 ? done(reported[0])
      : fail(new Error(`shard ${index}/${shards} exited ${code ?? signal} with ${reported.length} reports`))))
  }))).catch((error: unknown) => {
    // A failed run prints no receipt, so its siblings must stop rewriting files at once.
    for (const child of children) child.kill()
    throw error
  })
}

/**
 * Write every square once and sum the write counters plus the caller's `tally`.
 * A CLI parent first writes the square of median size itself. Its start-to-walk seconds and that
 * square's seconds per byte decide the shard count; with more than one shard it re-spawns its own
 * command line per shard, and each child walks the squares it owns except the sampled one.
 * `squares` must therefore be the same list in every process: derive it from the directory tree and
 * the source only, never from file contents a sibling shard is rewriting. One loader shards one walk.
 */
export async function writeRoadSquaresAcrossShards(
  preparedDirectory: string,
  squares: readonly string[],
  tally: Record<string, number>,
  writeSquare: (roadsArrowPath: string) => Promise<WriteRoadResult>,
): Promise<SquareWriteCounters> {
  const counters = squareWriteCounters()
  const write = async (square: string): Promise<void> => {
    const result = await writeSquare(resolve(preparedDirectory, square, 'roads.arrow'))
    counters.squares++
    counters.rows += result.rows
    counters.matched += result.matched
    counters.retracted += result.retracted
    counters.skipped += result.skipped
    counters.skippedForeign += result.skippedForeign
    if (result.updated) counters.squaresUpdated++
  }
  if (ownSquareShard || (fanOutFromThisCli && squares.length > 1)) {
    if (shardedWalkStarted) throw new Error('a road loader may shard one square walk only')
    shardedWalkStarted = true
  }
  let owned = squares
  if (ownSquareShard) {
    owned = shardSquares(squares, ownSquareShard).filter(square => square !== process.env[SAMPLED_SQUARE_ENV])
  } else if (shardedWalkStarted) {
    const loadSeconds = process.uptime()
    const bytes = new Map(squares.map(square => [square, statSync(resolve(preparedDirectory, square, 'roads.arrow')).size]))
    const sampledSquare = [...squares].sort((a, b) => bytes.get(a)! - bytes.get(b)!)[squares.length >> 1]
    owned = squares.filter(square => square !== sampledSquare)
    const started = performance.now()
    await write(sampledSquare)
    const walkSeconds = (performance.now() - started) / 1000 / bytes.get(sampledSquare)! *
      owned.reduce((sum, square) => sum + bytes.get(square)!, 0)
    const shards = squareShardCount(owned.length, loadSeconds, walkSeconds)
    console.error(JSON.stringify({ squareShards: shards, loadSeconds, estimatedWalkSeconds: walkSeconds }))
    if (shards > 1) {
      for (const totals of await spawnSquareShards(shards, sampledSquare)) {
        for (const key of Object.keys(totals.counters) as (keyof ShardTotals['counters'])[]) counters[key] += totals.counters[key]
        for (const key of Object.keys(tally)) tally[key] += totals.tally[key]
      }
      if (counters.squares !== squares.length) {
        throw new Error(`shards wrote ${counters.squares} of ${squares.length} squares under ${preparedDirectory}`)
      }
      return { ...counters, shards }
    }
  }
  for (const square of owned) await write(square)
  if (ownSquareShard) {
    if (!process.send) throw new Error(`${OWN_SQUARE_SHARD_ENV} is set but this process has no IPC channel`)
    await new Promise<void>((done, fail) =>
      process.send!({ counters, tally } satisfies ShardTotals, (error: Error | null) => (error ? fail(error) : done())))
  }
  return { ...counters, shards: 1 }
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
