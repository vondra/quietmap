/** Fill unbranched road chains from source topology: every square closes its own chains in parallel; the parent joins only chains that cross squares. */

import { resolve } from 'node:path'
import { fork, type ChildProcess } from 'node:child_process'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { parseArgs } from 'node:util'
import { withArrowWrite } from './lib/provenance.js'
import { applyRoadAadt, type WriteRoadResult } from './lib/roads-arrow.js'
import { FILLABLE, chainContinuesThroughEndpoint, joinAnchorAgreements, type Flow } from './lib/roads-continuity-plan.js'
import {
  endpointKeysHomedInOtherSquares, planSquareContinuity,
  type ContinuityFill, type SharedChainPart, type SharedEndpoint, type SquareContinuity,
} from './lib/roads-continuity-square.js'
import { SOURCE_ID_ROAD_CONTINUITY_HEURISTIC } from './lib/sources.js'
import { listPreparedSquares } from './lib/prepared-grid.js'
import { workerCount } from './lib/square-pool.js'

type SquareTask = { square: string } & ({ step: 'shared-endpoints' } | { step: 'plan'; keysTouchedFromOtherSquares: string[] } | { step: 'write'; fills: ContinuityFill[] })
type SquareReply = { result: unknown; error?: never } | { error: string; result?: never }
type PlacedChainPart = SharedChainPart & { square: string }

async function writeSquare(prepared: string, square: string, fills: ContinuityFill[]): Promise<WriteRoadResult> {
  let result!: WriteRoadResult
  const planned = new Map<number, Flow>()
  for (const { rows, flow } of fills) for (const row of rows) planned.set(row, flow)
  const path = resolve(prepared, square, 'roads.arrow')
  await withArrowWrite(path, table => {
    const applied = applyRoadAadt(table, path, (_row, index) => {
      const flow = planned.get(index)
      return flow ? { ...flow, sourceId: SOURCE_ID_ROAD_CONTINUITY_HEURISTIC, estimatedClasses: 15 } : null
    }, undefined, FILLABLE,
    { sourceIds: [SOURCE_ID_ROAD_CONTINUITY_HEURISTIC], when: (_row, index) => !planned.has(index) })
    result = applied.result
    return applied.table
  })
  return result
}

function requestSquare(worker: ChildProcess, task: SquareTask): Promise<SquareReply> {
  return new Promise(resolve => {
    const complete = (reply: SquareReply) => {
      worker.off('message', message); worker.off('error', error); worker.off('exit', exit)
      resolve(reply)
    }
    const message = (reply: unknown) => complete(reply as SquareReply)
    const error = (failure: Error) => complete({ error: failure.message })
    const exit = (code: number | null) => complete({ error: `continuity worker exited ${code} while processing ${task.square}` })
    worker.once('message', message); worker.once('error', error); worker.once('exit', exit)
    try { worker.send(task, failure => { if (failure) error(failure) }) }
    catch (failure) { error(failure instanceof Error ? failure : new Error(String(failure))) }
  })
}

/** Squares are independent within a step, so a free worker takes the next task and results arrive in completion order. */
async function* squareResults<Result>(prepared: string, tasks: Iterable<SquareTask>, taskCount: number): AsyncGenerator<{ task: SquareTask; result: Result }> {
  // An 898k-row square measured 1.3 GiB while reading and 0.9 GiB while writing; planning adds the square's endpoint map.
  const count = Math.min(taskCount, workerCount(process.env, 4 * 1024 ** 3))
  const workers = Array.from({ length: count }, () => fork(fileURLToPath(import.meta.url), [prepared], {
    serialization: 'advanced', stdio: ['ignore', 'ignore', 'inherit', 'ipc'],
  }))
  try {
    const pending = new Map<number, Promise<{ slot: number; task: SquareTask; reply: SquareReply }>>(), queued = tasks[Symbol.iterator]()
    const queue = (slot: number) => {
      const next = queued.next()
      if (next.done) pending.delete(slot)
      else pending.set(slot, requestSquare(workers[slot], next.value).then(reply => ({ slot, task: next.value, reply })))
    }
    for (let slot = 0; slot < count; slot++) queue(slot)
    for (let completed = 1; pending.size; completed++) {
      const { slot, task, reply } = await Promise.race(pending.values())
      if (reply.error !== undefined) throw new Error(reply.error)
      queue(slot)
      yield { task, result: reply.result as Result }
      if (completed % 1000 === 0 || completed === taskCount) console.log(JSON.stringify({ phase: `continuity-${task.step}`, completed, selected: taskCount }))
    }
  } finally {
    for (const worker of workers) worker.kill()
  }
}

export async function writeContinuitySquares(prepared: string, fillsBySquare: ReadonlyMap<string, ContinuityFill[]>) {
  const counts = { matched: 0, retracted: 0, updated: false }
  const tasks = [...fillsBySquare].map(([square, fills]): SquareTask => ({ step: 'write', square, fills }))
  for await (const { result } of squareResults<WriteRoadResult>(prepared, tasks, tasks.length)) {
    counts.matched += result.matched; counts.retracted += result.retracted
    counts.updated ||= result.updated
  }
  return counts
}

/** Chain parts meeting where exactly two piece ends share an endpoint form one chain; its anchors agree on one flow or conflict once. */
function joinSharedChainParts(parts: PlacedChainPart[], endpoints: ReadonlyMap<string, Pick<SharedEndpoint, 'pieceEnds' | 'chainEnds'>>) {
  const chainOf = parts.map((_, part) => part)
  const chain = (part: number): number => {
    while (chainOf[part] !== part) part = chainOf[part] = chainOf[chainOf[part]]
    return part
  }
  for (const [key, { pieceEnds, chainEnds: [first, second] }] of endpoints) {
    // An unsigned count on a side arm does not determine any turning movement.
    if (pieceEnds === 2 && second && chainContinuesThroughEndpoint(first.road, second.road, key)) chainOf[chain(first.part)] = chain(second.part)
  }
  const chains = new Map<number, PlacedChainPart[]>()
  for (const [part, member] of parts.entries()) {
    const members = chains.get(chain(part))
    if (members) members.push(member)
    else chains.set(chain(part), [member])
  }
  let anchors = 0, conflicts = 0
  const fills: Array<{ square: string; fill: ContinuityFill }> = []
  for (const members of chains.values()) {
    const agreement = joinAnchorAgreements(members.map(member => member.agreement))
    anchors += agreement.anchors; conflicts += agreement.conflicts
    if (agreement.flow) for (const { square, fillRows } of members) if (fillRows.length) fills.push({ square, fill: { rows: fillRows, flow: agreement.flow } })
  }
  return { anchors, conflicts, fills }
}

export async function enrichContinuityDirectory(preparedDirectory: string) {
  const prepared = resolve(preparedDirectory)
  const squares = listPreparedSquares(prepared, [-90, -180, 90, 180], 'roads.arrow')
  if (!squares.length) throw new Error(`${prepared}: no prepared road scope`)
  const keysTouchedFromOtherSquares = new Map<string, string[]>()
  for await (const { result } of squareResults<Map<string, string[]>>(prepared,
    squares.map(square => ({ step: 'shared-endpoints', square })), squares.length)) {
    for (const [home, keys] of result) {
      const known = keysTouchedFromOtherSquares.get(home)
      if (known) for (const key of keys) known.push(key)
      else keysTouchedFromOtherSquares.set(home, keys)
    }
  }
  const fillsBySquare = new Map<string, ContinuityFill[]>(), parts: PlacedChainPart[] = []
  const endpoints = new Map<string, Pick<SharedEndpoint, 'pieceEnds' | 'chainEnds'>>()
  let rows = 0, anchors = 0, conflicts = 0
  const planTasks = function* (): Generator<SquareTask> {
    for (const square of squares) {
      yield { step: 'plan', square, keysTouchedFromOtherSquares: keysTouchedFromOtherSquares.get(square) ?? [] }
      keysTouchedFromOtherSquares.delete(square)
    }
  }
  for await (const { task: { square }, result } of squareResults<SquareContinuity>(prepared, planTasks(), squares.length)) {
    rows += result.rows; anchors += result.anchors; conflicts += result.conflicts
    if (result.hasOwned || result.fills.length) fillsBySquare.set(square, result.fills)
    for (const { key, pieceEnds, chainEnds } of result.sharedEndpoints) {
      const known = endpoints.get(key) ?? { pieceEnds: 0, chainEnds: [] }
      known.pieceEnds += pieceEnds
      // Beyond two piece ends nothing continues, so the chain ends themselves no longer matter.
      known.chainEnds = known.pieceEnds > 2 ? [] : [...known.chainEnds, ...chainEnds.map(end => ({ ...end, part: end.part + parts.length }))]
      endpoints.set(key, known)
    }
    for (const part of result.sharedChainParts) parts.push({ ...part, square })
  }
  const joined = joinSharedChainParts(parts, endpoints)
  for (const { square, fill } of joined.fills) fillsBySquare.set(square, [...fillsBySquare.get(square) ?? [], fill])
  console.log(JSON.stringify({ phase: 'continuity-joined', sharedEndpoints: endpoints.size, sharedChainParts: parts.length, writeSquares: fillsBySquare.size, total: squares.length, heapBytes: process.memoryUsage().heapUsed }))
  return { rows, ...await writeContinuitySquares(prepared, fillsBySquare), anchors: anchors + joined.anchors, conflicts: conflicts + joined.conflicts }
}

async function main(): Promise<void> {
  const { values } = parseArgs({ options: { 'prepared-dir': { type: 'string' } } })
  if (!values['prepared-dir']) throw new Error('usage: enrich-roads-continuity-fill.ts --prepared-dir PREPARED_YEAR_DIR')
  console.log(JSON.stringify(await enrichContinuityDirectory(values['prepared-dir'])))
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  if (process.send) {
    const prepared = process.argv[2]
    process.on('message', async (task: SquareTask) => {
      try {
        process.send!({ result: task.step === 'shared-endpoints' ? endpointKeysHomedInOtherSquares(prepared, task.square)
          : task.step === 'plan' ? planSquareContinuity(prepared, task.square, task.keysTouchedFromOtherSquares)
            : await writeSquare(prepared, task.square, task.fills) })
      } catch (error) { process.send!({ error: error instanceof Error ? error.message : String(error) }) }
    })
    process.once('disconnect', () => process.exit(0))
  } else {
    main().catch((error: unknown) => { console.error(error); process.exitCode = 1 })
  }
}
