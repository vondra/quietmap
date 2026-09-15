/** Bounded parallel TMAS writes: one process owns each prepared road file. */

import { fork } from 'node:child_process'
import { statSync } from 'node:fs'
import { resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { getHeapStatistics } from 'node:v8'
import { applyTmasProfileSquares } from '../enrich-roads-us.js'
import type { TmasStationProfile } from './roads-us-tmas-source.js'
import { workerCount } from './square-pool.js'

type Result = Awaited<ReturnType<typeof applyTmasProfileSquares>>
interface Shard { squares: string[]; stations: readonly TmasStationProfile[] }

export async function writeTmasProfileSquares(
  prepared: string, squares: readonly string[], stations: readonly TmasStationProfile[],
): Promise<Result> {
  // As for railway Arrow workers, reserve another heap-equivalent for native buffers.
  const count = Math.min(squares.length, workerCount(process.env, 2 * getHeapStatistics().heap_size_limit))
  if (count <= 1) return applyTmasProfileSquares(prepared, squares, stations)
  const shards = Array.from({ length: count }, () => ({ squares: [] as string[], bytes: 0 }))
  const sized = squares.map(square => ({ square, bytes: statSync(resolve(prepared, square, 'roads.arrow')).size }))
  for (const { square, bytes } of sized.sort((a, b) => b.bytes - a.bytes)) {
    const shard = shards.reduce((a, b) => a.bytes <= b.bytes ? a : b)
    shard.squares.push(square)
    shard.bytes += bytes
  }
  const children: ReturnType<typeof fork>[] = []
  try {
    const results = await Promise.all(shards.map(shard => new Promise<Result>((accept, reject) => {
      const child = fork(fileURLToPath(import.meta.url), [prepared], {
        stdio: ['ignore', 'inherit', 'inherit', 'ipc'], serialization: 'advanced',
      })
      children.push(child)
      let result: Result | undefined
      child.once('error', reject)
      child.once('message', message => { result = message as Result })
      child.once('close', (code, signal) => {
        if (code !== 0 || !result) reject(new Error(`TMAS writer exited ${code ?? signal} without a result`))
        else accept(result)
      })
      child.send({ squares: shard.squares, stations } satisfies Shard, error => { if (error) reject(error) })
    })))
    return results.reduce((total, result) => ({ rows: total.rows + result.rows,
      matched: total.matched + result.matched, squaresUpdated: total.squaresUpdated + result.squaresUpdated }),
    { rows: 0, matched: 0, squaresUpdated: 0 })
  } finally {
    const closed = children.map(child => child.exitCode !== null || child.signalCode !== null
      ? Promise.resolve() : new Promise<void>(done => child.once('close', () => done())))
    for (const child of children) if (child.exitCode === null && child.signalCode === null) child.kill()
    await Promise.all(closed)
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  process.once('message', async (message: Shard) => {
    try {
      const result = await applyTmasProfileSquares(process.argv[2], message.squares, message.stations)
      process.send!(result, error => {
        if (error) { console.error(error); process.exitCode = 1 }
        process.disconnect()
      })
    } catch (error) {
      console.error(error)
      process.exitCode = 1
      process.disconnect()
    }
  })
}
