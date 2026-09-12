/** Run one per-square heuristic over every prepared square, sharded across QM_ROAD_WORKERS child processes. */

import { spawn } from 'node:child_process'
import { resolve } from 'node:path'
import { parseArgs } from 'node:util'
import { listPreparedSquares } from './prepared-grid.js'
import { cpuJobs, fitJobs } from './worker-jobs.js'

/** One Node square-heuristic process; a 20 GiB layer cgroup holds 20. */
export const SQUARE_WORKER_BYTES = 1 << 30

export function workerCount(env: NodeJS.ProcessEnv = process.env): number {
  const raw = env.QM_ROAD_WORKERS
  const requested = raw === undefined || raw === '' ? cpuJobs() : Number(raw)
  if (!Number.isInteger(requested) || requested < 1) throw new Error('QM_ROAD_WORKERS must be a positive integer')
  return fitJobs(requested, SQUARE_WORKER_BYTES)
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
export async function fanOutIfNeeded(): Promise<boolean> {
  if (argvHasShard()) return false
  const workers = workerCount()
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
export async function runSquareSteps(usage: string, step: (squareDirectory: string) => Promise<object>): Promise<void> {
  const { values } = parseArgs({ options: { 'prepared-dir': { type: 'string' }, shard: { type: 'string' } } })
  if (!values['prepared-dir']) throw new Error(usage)
  const directory = resolve(values['prepared-dir'])
  if (await fanOutIfNeeded()) return
  const squares = listPreparedSquares(directory, [-90, -180, 90, 180])
  if (!squares.length) throw new Error(`${directory}: no prepared road scope`)
  for (const square of shardSquares(squares, parseShard(values.shard))) {
    console.log(JSON.stringify({ square, ...(await step(resolve(directory, square))) }))
  }
}
