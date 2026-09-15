/** Fill unbranched road chains across all storage owners using source topology and bounded-memory SQLite. */

import { mkdtempSync, readFileSync, rmSync, statSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { DatabaseSync, type SQLInputValue } from 'node:sqlite'
import { fork, type ChildProcess } from 'node:child_process'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { parseArgs } from 'node:util'
import { DataType, tableFromIPC } from 'apache-arrow'
import { withArrowWrite } from './lib/provenance.js'
import { applyRoadAadt } from './lib/roads-arrow.js'
import { readPlanningRoads } from './lib/road-planning-input.js'
import { roadContinuityComponent, planContinuityComponent, FILLABLE, type ContinuityRoad } from './lib/roads-continuity-plan.js'
import { SOURCE_ID_ROAD_CONTINUITY_HEURISTIC, SOURCES, isMeasured } from './lib/sources.js'
import { listPreparedSquares } from './lib/prepared-grid.js'
import { SourceTransportTopology, transportPieceKey } from './lib/transport-topology.js'
import { workerCount } from './lib/square-pool.js'

interface StoredRoad extends Omit<ContinuityRoad, 'aadt' | 'roundabout'> {
  square: string; row_index: number; light: number; medium: number; heavy: number; moto: number; roundabout: number
}
function decodeRoad(row: StoredRoad): ContinuityRoad & Pick<StoredRoad, 'square' | 'row_index'> {
  const { light, medium, heavy, moto, roundabout, ...road } = row
  return { ...road, roundabout: roundabout !== 0, aadt: [light, medium, heavy, moto] }
}

type StagedRoads = SQLInputValue[][]
type SquareReply = { roads: StagedRoads; error?: never } | { error: string; roads?: never }

function prepareSquare(prepared: string, square: string, topology: SourceTransportTopology): StagedRoads {
  const table = tableFromIPC(readFileSync(resolve(prepared, square, 'roads.arrow')))
  if (table.schema.metadata.get('road_traffic_contract') === '1') throw new Error('road continuity must precede final allocation')
  const direction = table.getChild('oneway')
  if (!direction || !DataType.isInt(direction.type) || direction.type.bitWidth !== 8 || direction.type.isSigned || direction.nullCount) {
    throw new Error(`${square}: invalid oneway direction column`)
  }
  const identities = topology.squarePieces(square)
  const roads = readPlanningRoads(table)
  const staged: StagedRoads = []
  for (const road of roads) {
    const key = transportPieceKey(String(road.osmId), road.segIdx), identity = identities.get(key)
    if (!identity) throw new Error(`${square}: source topology missing or repeated road piece ${key}`)
    identities.delete(key)
    const oneway = Number(direction.get(road.i))
    if (oneway > 2) throw new Error(`${square}: invalid oneway direction ${oneway}`)
    const { osmId, cls, src, ref, aadt, access, roundabout, countBasis, observationId, observationSourceId } = road
    staged.push([square, road.i, identity.startKey, identity.endKey, osmId, cls, src, ref, ...aadt, access, Number(roundabout), oneway, countBasis, observationId, observationSourceId])
  }
  if (identities.size) throw new Error(`${square}: ${identities.size} source road pieces absent from Arrow`)
  return staged
}

function requestSquare(worker: ChildProcess, square: string): Promise<SquareReply> {
  return new Promise(resolve => {
    const complete = (reply: SquareReply) => {
      worker.off('message', message); worker.off('error', error); worker.off('exit', exit)
      resolve(reply)
    }
    const message = (reply: unknown) => complete(reply as SquareReply)
    const error = (failure: Error) => complete({ error: failure.message })
    const exit = (code: number | null) => complete({ error: `continuity worker exited ${code} while preparing ${square}` })
    worker.once('message', message); worker.once('error', error); worker.once('exit', exit)
    try { worker.send(square, failure => { if (failure) error(failure) }) }
    catch (failure) { error(failure instanceof Error ? failure : new Error(String(failure))) }
  })
}

async function* preparedSquares(prepared: string, squares: string[]): AsyncGenerator<StagedRoads> {
  // The measured 898k-row sample uses 1.3 GiB in its worker plus 0.9 GiB in the writer.
  const count = Math.min(squares.length, workerCount(process.env, 4 * 1024 ** 3))
  const workers = Array.from({ length: count }, () => fork(fileURLToPath(import.meta.url), [prepared], {
    serialization: 'advanced', stdio: ['ignore', 'ignore', 'inherit', 'ipc'],
  }))
  try {
    const pending = workers.map((worker, index) => requestSquare(worker, squares[index]))
    for (let position = 0; position < squares.length; position++) {
      const slot = position % count, reply = await pending[slot]
      if (reply.error !== undefined) throw new Error(reply.error)
      yield reply.roads
      // A worker receives another square only after the writer consumed its previous result.
      if (position + count < squares.length) pending[slot] = requestSquare(workers[slot], squares[position + count])
    }
  } finally {
    for (const worker of workers) worker.kill()
  }
}

export async function enrichContinuityDirectory(preparedDirectory: string) {
  const prepared = resolve(preparedDirectory)
  const squares = listPreparedSquares(prepared, [-90, -180, 90, 180], 'roads.arrow')
  if (!squares.length) throw new Error(`${prepared}: no prepared road scope`)
  using topology = new SourceTransportTopology(prepared, 'roads')
  const available = new Set(squares)
  for (const square of topology.squares()) {
    if (!available.has(square)) throw new Error(`source topology road owner is missing: ${square}`)
  }
  const temporary = mkdtempSync(join(tmpdir(), 'road-continuity-'))
  try {
    using database = new DatabaseSync(join(temporary, 'graph.sqlite'))
    // Disposable staging only: a failure is rebuilt from unchanged authored inputs.
    database.exec(`PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF;
      CREATE TABLE roads (i INTEGER PRIMARY KEY, square TEXT, row_index INTEGER, a TEXT, b TEXT,
        osmId INTEGER, cls INTEGER, src INTEGER, ref TEXT, light REAL, medium REAL, heavy REAL, moto REAL,
        access INTEGER, roundabout INTEGER, direction INTEGER, countBasis TEXT, observationId TEXT, observationSourceId INTEGER, visited INTEGER DEFAULT 0);
      CREATE TABLE fills (square TEXT, row_index INTEGER, light REAL, medium REAL, heavy REAL, moto REAL, countBasis TEXT, observationId TEXT, observationSourceId INTEGER,
                          PRIMARY KEY(square,row_index)) WITHOUT ROWID;`)
    const insert = database.prepare(`INSERT INTO roads(square,row_index,a,b,osmId,cls,src,ref,light,medium,heavy,moto,access,roundabout,direction,countBasis,observationId,observationSourceId)
      VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)`)
    let rows = 0, position = 0
    for await (const roads of preparedSquares(prepared, squares)) {
      database.exec('BEGIN')
      for (const road of roads) insert.run(...road)
      database.exec('COMMIT')
      rows += roads.length
      position++
      if (position % 1000 === 0) console.log(JSON.stringify({ phase: 'continuity-inputs', squares: position, rows }))
    }
    const measuredSources = SOURCES.filter(source => isMeasured(source.id)).map(source => source.id)
    // A component without an observation cannot produce a fill; retain all rows for branch degree.
    const pending = `visited=0 AND cls IN (${[...FILLABLE].join(',')})
      AND src IN (${measuredSources.join(',')}) AND observationId != ''`
    database.exec(`CREATE INDEX roads_a ON roads(a); CREATE INDEX roads_b ON roads(b);
      CREATE INDEX roads_pending ON roads(i) WHERE ${pending};`)
    const next = database.prepare(`SELECT * FROM roads WHERE ${pending} LIMIT 1`)
    const incident = database.prepare('SELECT * FROM roads WHERE a=? OR b=? LIMIT 3')
    const visited = database.prepare('UPDATE roads SET visited=1 WHERE i=?')
    const fill = database.prepare('INSERT INTO fills VALUES (?,?,?,?,?,?,?,?,?)')
    let anchors = 0, conflicts = 0, components = 0, processedRows = 0
    for (let seed; (seed = next.get() as unknown as StoredRoad | undefined);) {
      const component = roadContinuityComponent(decodeRoad(seed), endpoint =>
        (incident.all(endpoint, endpoint) as unknown as StoredRoad[]).flatMap(row => {
          const road = decodeRoad(row)
          return road.a === endpoint && road.b === endpoint ? [road, road] : [road]
        }))
      const plan = planContinuityComponent(component)
      anchors += plan.anchors; conflicts += plan.conflicts
      database.exec('BEGIN')
      for (const road of component) {
        visited.run(road.i)
        const flow = plan.fill.get(road.i)
        if (!flow) continue
        fill.run(road.square, road.row_index, flow.light, flow.medium, flow.heavy, flow.moto, flow.countBasis, flow.observationId, flow.observationSourceId)
      }
      database.exec('COMMIT')
      components++; processedRows += component.length
      if (components % 10000 === 0) console.log(JSON.stringify({ phase: 'continuity-components', components, processedRows, anchors, conflicts }))
    }
    const bySquare = database.prepare('SELECT * FROM fills WHERE square=?')
    const counts = { rows, matched: 0, retracted: 0, updated: false, anchors, conflicts,
      graphBytes: statSync(join(temporary, 'graph.sqlite')).size }
    for (const square of squares) {
      const planned = new Map(bySquare.all(square).map(row => [row.row_index as number, row]))
      const path = resolve(prepared, square, 'roads.arrow')
      await withArrowWrite(path, table => {
        const applied = applyRoadAadt(table, path, (_row, index) => {
          const flow = planned.get(index)
          return flow ? { light: Number(flow.light), medium: Number(flow.medium), heavy: Number(flow.heavy), moto: Number(flow.moto),
            sourceId: SOURCE_ID_ROAD_CONTINUITY_HEURISTIC, countBasis: flow.countBasis as ContinuityRoad['countBasis'], observationId: String(flow.observationId),
            observationSourceId: Number(flow.observationSourceId), estimatedClasses: 15 } : null
        }, undefined, FILLABLE,
        { sourceIds: [SOURCE_ID_ROAD_CONTINUITY_HEURISTIC], when: (_row, index) => !planned.has(index) })
        counts.matched += applied.result.matched; counts.retracted += applied.result.retracted
        counts.updated ||= applied.result.updated
        return applied.table
      })
    }
    return counts
  } finally {
    rmSync(temporary, { recursive: true, force: true })
  }
}

async function main(): Promise<void> {
  const { values } = parseArgs({ options: { 'prepared-dir': { type: 'string' } } })
  if (!values['prepared-dir']) throw new Error('usage: enrich-roads-continuity-fill.ts --prepared-dir PREPARED_YEAR_DIR')
  console.log(JSON.stringify(await enrichContinuityDirectory(values['prepared-dir'])))
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  if (process.send) {
    const topology = new SourceTransportTopology(process.argv[2], 'roads')
    process.on('message', (square: string) => {
      try { process.send!({ roads: prepareSquare(process.argv[2], square, topology) }) }
      catch (error) { process.send!({ error: error instanceof Error ? error.message : String(error) }) }
    })
    process.once('disconnect', () => { topology[Symbol.dispose](); process.exit(0) })
  } else {
    main().catch((error: unknown) => { console.error(error); process.exitCode = 1 })
  }
}
