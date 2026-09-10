//! Changed measurements must invalidate obsolete continuity fills while respecting source priority.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { Table, vectorFromArray, tableFromIPC, tableToIPC, Float64, Int32, Uint8, Uint16, Utf8 } from 'apache-arrow'
import { classDefaultTotal } from './lib/road-class-defaults.js'
import { fillRoadContinuity } from './enrich-roads-continuity-fill.js'
import { SOURCE_ID_EU_CITY_TRAFFIC as MEASURED, SOURCE_ID_ROAD_CONTINUITY_HEURISTIC as FILL,
  SOURCE_ID_OSM_TRANSITION_TAPER as TAPER } from './lib/source-ids.generated.js'

test('reruns replace or retract their own fills when measurements change, reclaim lower-priority tapers, then leave files untouched', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'road-continuity-'))
  try {
    for (const anchor of [8000, 200]) {
      const path = join(dir, `${anchor}.arrow`)
      const table = new Table({
        ref: vectorFromArray(['D1', 'D1', 'D2', 'D3', 'D1'], new Utf8()),
        start_lat: vectorFromArray([50, 50.001, 51, 52, 50.002], new Float64()),
        end_lat: vectorFromArray([50.001, 50.002, 51.001, 52.001, 50.003], new Float64()),
        start_lon: vectorFromArray([14, 14, 14, 14, 14], new Float64()),
        end_lon: vectorFromArray([14, 14, 14, 14, 14], new Float64()),
        road_class: vectorFromArray([4, 4, 4, 4, 4], new Uint8()),
        source_id: vectorFromArray([MEASURED, FILL, FILL, MEASURED, TAPER], new Uint16()),
        aadt_light: vectorFromArray([anchor, 6000, 7000, 1500, 300], new Int32()),
        aadt_medium: vectorFromArray([0, 100, 200, 0, 0], new Int32()),
        aadt_heavy: vectorFromArray([0, 100, 200, 0, 0], new Int32()),
        aadt_moto: vectorFromArray([0, 10, 20, 0, 0], new Int32()),
        speed_taper: vectorFromArray([0, 0, 70, 0, 60], new Uint8()),
      })
      writeFileSync(path, Buffer.from(tableToIPC(table, 'file')))
      const supported = anchor > classDefaultTotal(4)
      const filled = supported ? 2 : 0
      assert.deepEqual(await fillRoadContinuity(path), { filled, conflicts: 0, updated: true, retracted: supported ? 1 : 2 })
      const actual = tableFromIPC(readFileSync(path))
      const column = (name: string) => Array.from(actual.getChild(name)!)
      assert.deepEqual(column('source_id'), [MEASURED, supported ? FILL : 0, 0, MEASURED, supported ? FILL : TAPER])
      assert.deepEqual(column('aadt_light'), [anchor, supported ? anchor : 0, 0, 1500, supported ? anchor : 300])
      for (const name of ['aadt_medium', 'aadt_heavy', 'aadt_moto']) {
        assert.deepEqual(column(name), [0, 0, 0, 0, 0], name)
      }
      assert.deepEqual(column('speed_taper'), [0, 0, 0, 0, supported ? 0 : 60])
      const before = readFileSync(path)
      const stat = statSync(path, { bigint: true })
      assert.deepEqual(await fillRoadContinuity(path), { filled, conflicts: 0, updated: false, retracted: 0 })
      assert.deepEqual(readFileSync(path), before)
      assert.equal(statSync(path, { bigint: true }).ino, stat.ino)
      assert.equal(statSync(path, { bigint: true }).mtimeNs, stat.mtimeNs)
    }
  } finally { rmSync(dir, { recursive: true, force: true }) }
})
