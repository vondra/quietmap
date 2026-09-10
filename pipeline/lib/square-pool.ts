/** Run one per-square road heuristic over every prepared square, sharded across QM_ROAD_WORKERS child processes. */

import { spawn } from 'node:child_process'
import { resolve } from 'node:path'
import { parseArgs } from 'node:util'
import { listPreparedSquares } from './prepared-grid.js'

export function workerCount(env: NodeJS.ProcessEnv = process.env): number {
  const workers = Number(env.QM_ROAD_WORKERS ?? 1)
  if (!Number.isInteger(workers) || workers < 1) throw new Error('QM_ROAD_WORKERS must be a positive integer')
  return workers
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

/**
 * Entry point shared by the per-square heuristics: `--prepared-dir DIR [--shard i/n]`.
 * Without `--shard`, QM_ROAD_WORKERS > 1 re-spawns this same script once per shard
 * and inherits their stdout, so the chain log keeps one line per square.
 */
export async function runSquareSteps(usage: string, step: (squareDirectory: string) => Promise<object>): Promise<void> {
  const { values } = parseArgs({ options: { 'prepared-dir': { type: 'string' }, shard: { type: 'string' } } })
  if (!values['prepared-dir']) throw new Error(usage)
  const directory = resolve(values['prepared-dir'])
  const squares = listPreparedSquares(directory, [-90, -180, 90, 180])
  if (!squares.length) throw new Error(`${directory}: no prepared road scope`)
  const workers = workerCount()
  if (values.shard === undefined && workers > 1) {
    await Promise.all(Array.from({ length: workers }, (_, index) => new Promise<void>((done, fail) => {
      const child = spawn(process.execPath,
        [...process.execArgv, process.argv[1], '--prepared-dir', directory, '--shard', `${index}/${workers}`],
        { stdio: 'inherit', env: { ...process.env, QM_ROAD_WORKERS: '1' } })
      child.on('error', fail)
      child.on('exit', code => (code === 0 ? done() : fail(new Error(`shard ${index}/${workers} exited ${code}`))))
    })))
    return
  }
  for (const square of shardSquares(squares, parseShard(values.shard))) {
    console.log(JSON.stringify({ square, ...(await step(resolve(directory, square))) }))
  }
}
