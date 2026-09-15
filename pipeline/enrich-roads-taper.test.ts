/** Speed-transition topology and raw/final traffic preservation regressions. */

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { statSync, writeFileSync } from 'node:fs'
import { Float32, Float64, Int16, Int32, RecordBatch, Schema, Table, Uint8, Uint16, tableFromIPC, tableToIPC, vectorFromArray } from 'apache-arrow'
import { bytes, decodeQmBlocks, encodeQmBlocks, writeRoadsFixture } from './lib/road-test-fixture.js'
import { withArrowWrite } from './lib/provenance.js'
import { iso2Code } from './lib/prepared-grid.js'
import { enrichTaperSquare } from './enrich-roads-taper.js'
import { buildTaperPlan as buildPlanRaw, resolveSpeed, type CountrySpeeds, type Seg } from './lib/roads-taper-plan.js'
import { CZ_SPEEDS } from './lib/road-planning-defaults.generated.js'

// The CZ row from the SAME generated table the CLI uses — a legal-speed
// refresh flows into these tests automatically (drift-proof by design).
const CZ: CountrySpeeds = CZ_SPEEDS
const buildTaperPlan = (segs: Seg[]) => buildPlanRaw(segs, CZ)

/** Pure planner fixtures use opaque node labels; native readers supply z30 keys. */
function seg(o: Partial<Seg> & { i: number; osmId: number; a: string; b: string }): Seg {
  return {
    segIdx: 0, cls: 4, speedTag: 0, builtUp: 1, access: 0,
    roundabout: false, len: 60,
    ...o,
  }
}

/** A way as a straight chain of `n` segments from node `startN`, sharing nodes. */
function way(osmId: number, i0: number, startN: number, n: number, o: Partial<Seg> = {}): Seg[] {
  return Array.from({ length: n }, (_, k) =>
    seg({
      i: i0 + k, osmId, segIdx: k,
      a: `50.${startN + k}_14.0`, b: `50.${startN + k + 1}_14.0`,
      ...o,
    }))
}

test('speed-tag edge (DE shape): untagged side ramps from the tag toward its own default', () => {
  // Way 1: one tagged-100 segment. Way 2: 5 untagged urban segments (resolved 50).
  // Continuation node (exactly two ways) → boundary; step = 30·log10(100/50) ≈ 9 dB.
  const segs = [...way(1, 0, 0, 1, { speedTag: 100 }), ...way(2, 1, 1, 5, { builtUp: 2 })]
  const { plan, stats } = buildTaperPlan(segs)
  assert.equal(stats.boundaries, 1)
  assert.equal(stats.kindCounts['speed-tag-edge'], 1)
  assert.equal(plan.has(0), false, 'tagged row never planned')

  // Untagged rows at dMid 30/90/150/210 m of the 250 m window ramp 100 → 50.
  const speeds = [1, 2, 3, 4].map((i) => plan.get(i)?.speed)
  assert.deepEqual(speeds, [94, 82, 70, 58])
  assert.equal(plan.get(5)?.speed, undefined, 'past the window the own value wins (suppressed)')
})

test('built-up flip inside one way: both defaults ramp from the midpoint, W/2 each side', () => {
  // One way: 3 rural (90) + 3 urban (50) segments, class 4 → speed-only step.
  const segs = [...way(1, 0, 0, 3, { builtUp: 1 }), ...way(1, 3, 3, 3, { builtUp: 2 })]
  segs.forEach((s, k) => (s.segIdx = k))
  const { plan, stats } = buildTaperPlan(segs)
  assert.equal(stats.boundaries, 1)
  assert.equal(stats.kindCounts['built-up-flip'], 1)

  // 125 m window per side, midpoint anchor 70: rural side ramps 70→90,
  // urban side 70→50; row dMids 30/90 → t=0.24/0.72.
  assert.deepEqual([plan.get(2)?.speed, plan.get(1)?.speed], [75, 84], 'rural side ascends away from the flip')
  assert.deepEqual([plan.get(3)?.speed, plan.get(4)?.speed], [65, 56], 'urban side descends away from the flip')
  assert.equal(plan.get(0)?.speed, undefined, 'beyond W/2 suppressed')
  assert.equal(plan.get(5)?.speed, undefined, 'beyond W/2 suppressed')
})

test('a junction kills the boundary; a service driveway does not', () => {
  const speedEdge = (extra: Seg[]) => {
    const segs = [
      ...way(1, 0, 0, 1, { speedTag: 50 }),
      ...way(2, 1, 1, 4),
      ...extra,
    ]
    return buildTaperPlan(segs).stats.boundaries
  }
  // A third THROUGH way at the shared node "50.1_14.0" → junction → no boundary.
  const side = seg({ i: 90, osmId: 9, cls: 5, a: '50.1_14.0', b: '50.1_14.9' })
  assert.equal(speedEdge([side]), 0, 'through-class side road = junction')
  // A service driveway (class 7) at the same node is not a flow split.
  const driveway = seg({ i: 91, osmId: 9, cls: 7, a: '50.1_14.0', b: '50.1_14.9' })
  assert.equal(speedEdge([driveway]), 1, 'service driveway ignored')
})

test('a loop way meeting another way is physical degree 3 — junction, not continuation', () => {
  // Way 9 loops n1→n2→n3→n1; way 1 (tagged) ends at n1. Only 2 distinct way
  // ids at n1, but 3 segment endpoints — the edge-degree rule must refuse the
  // continuation link (a way-id set alone would walk straight through).
  const tagged = way(1, 0, 0, 1, { speedTag: 50 })
  const loop = [
    seg({ i: 10, osmId: 9, segIdx: 0, a: '50.1_14.0', b: '50.1_14.5' }),
    seg({ i: 11, osmId: 9, segIdx: 1, a: '50.1_14.5', b: '50.2_14.5' }),
    seg({ i: 12, osmId: 9, segIdx: 2, a: '50.2_14.5', b: '50.1_14.0' }),
  ]
  const { plan, stats } = buildTaperPlan([...tagged, ...loop])
  assert.equal(stats.boundaries, 0, 'no boundary across a degree-3 node')
  assert.equal(plan.size, 0)
})

test('never-touch invariants: restricted access and roundabouts are not planned', () => {
  const taggedRow = way(1, 0, 0, 1, { speedTag: 50 })
  const restricted = way(2, 1, 1, 4, { access: 3 })
  assert.equal(buildTaperPlan([...taggedRow, ...restricted]).plan.size, 0, 'destination-access rows ineligible')

  const roundabout = way(2, 1, 1, 4, { roundabout: true })
  assert.equal(buildTaperPlan([...taggedRow, ...roundabout]).plan.size, 0, 'roundabout rows ineligible')
})

test('resolveSpeed mirrors the engine cascade for the CZ row', () => {
  const s = (o: Partial<Seg>) => seg({ i: 0, osmId: 1, a: 'x', b: 'y', ...o })
  assert.equal(resolveSpeed(s({ speedTag: 70 }), CZ), 70, 'tag wins')
  assert.equal(resolveSpeed(s({ speedTag: 255 }), CZ), 130, 'derestricted sentinel')
  assert.equal(resolveSpeed(s({ cls: 4, builtUp: 2 }), CZ), 50, 'urban')
  assert.equal(resolveSpeed(s({ cls: 4, builtUp: 1 }), CZ), 90, 'rural')
  assert.equal(resolveSpeed(s({ cls: 4, builtUp: 0 }), CZ), 50, 'unknown → legacy table')
  assert.equal(resolveSpeed(s({ cls: 5, builtUp: 1 }), CZ), 30, 'residential stays legacy (engine scope)')
  assert.equal(resolveSpeed(s({ cls: 0 }), CZ), 130, 'motorway')
  assert.equal(resolveSpeed(s({ cls: 1 }), CZ), 110, 'CZ motorroad trunk')
})

for (const contract of ['0', '1']) test(`speed-only IPC preserves contract${contract} traffic bytes and retracts only obsolete speeds`, async () => {
  const path = writeRoadsFixture(`speed-only-contract${contract}.arrow`, [4, 4, 4, 4], {
    speeds: [100, 0, 0, 0], sourceIds: [10, 12, 0, 20],
    countryCodes: ['CZ', 'CZ', 'CZ', 'AT'].map(iso2Code),
  })
  await withArrowWrite(path, table => {
    const columns = Object.fromEntries(table.schema.fields.map(field => [field.name, table.getChild(field.name)!]))
    for (const axis of ['gx', 'gy']) {
      const starts = [...table.getChild(`start_${axis}`)!].map(Number)
      columns[`end_${axis}`] = vectorFromArray(starts.map((value, i) => starts[i + 1] ?? value + 1000), new Int32())
    }
    columns.segment_idx = vectorFromArray([0, 0, 0, 0], new Int16())
    columns.length_m = vectorFromArray([60, 60, 60, 60], new Float32())
    for (const name of ['access', 'junction']) columns[name] = vectorFromArray([0, 0, 0, 0], new Uint8())
    columns.built_up = vectorFromArray([2, 2, 2, 2], new Uint8())
    columns.traffic_estimated = vectorFromArray([0, 0, 15, 5], new Uint8())
    if (contract === '0') columns.traffic_observation_source = vectorFromArray([10, 12, 0, 20], new Uint16())
    else for (const name of ['traffic_count_basis', 'traffic_observation_id', 'traffic_observation_source']) delete columns[name]
    columns.speed_taper = vectorFromArray([0, 0, 77, 44], new Uint8())
    for (const [index, name] of ['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto'].entries()) {
      columns[name] = vectorFromArray([0, 0.125 * (index + 1), 10.25 + index, 1234567.875], new Float64())
    }
    return new Table(columns)
  })
  const table = tableFromIPC(bytes(path))
  const envelope = decodeQmBlocks(table.schema.metadata.get('qm_blocks')!)[0].bbox
  const schema = new Schema(table.schema.fields, new Map([...table.schema.metadata,
    ['qm_blocks', encodeQmBlocks([envelope, envelope])],
    ['road_traffic_contract', contract], ['fixture', 'metadata must survive unchanged']]))
  writeFileSync(path, tableToIPC(new Table(schema, [table.slice(0, 2), table.slice(2)]
    .flatMap(part => part.batches.map(batch => new RecordBatch(schema, batch.data)))), 'file'))
  const input = tableFromIPC(bytes(path))
  assert.equal((await enrichTaperSquare(path)).updated, true)
  let output = tableFromIPC(bytes(path))
  assert.deepEqual([...output.getChild('speed_taper')!], [0, 94, 82, 44])
  assert.deepEqual([...output.schema.metadata], [...input.schema.metadata])
  assert.deepEqual(output.batches.map(batch => batch.numRows), input.batches.map(batch => batch.numRows))
  for (const field of input.schema.fields.filter(field => field.name !== 'speed_taper')) {
    assert.deepEqual(output.schema.fields.find(candidate => candidate.name === field.name), field)
    const before = input.getChild(field.name)!, after = output.getChild(field.name)!
    assert.deepEqual([...after], [...before], field.name)
    if (field.name.startsWith('aadt_')) {
      for (let index = 0; index < before.data.length; index++) {
        const a = after.data[index].values, b = before.data[index].values
        assert.deepEqual(Buffer.from(a.buffer, a.byteOffset, a.byteLength), Buffer.from(b.buffer, b.byteOffset, b.byteLength), field.name)
      }
    }
  }
  const stable = bytes(path), mtime = statSync(path).mtimeMs
  assert.equal((await enrichTaperSquare(path)).updated, false)
  assert.deepEqual(bytes(path), stable)
  assert.equal(statSync(path).mtimeMs, mtime)

  await withArrowWrite(path, current => current.setChild('speed_limit', vectorFromArray([0, 0, 0, 0], new Uint8())))
  const retiredInput = tableFromIPC(bytes(path))
  assert.equal((await enrichTaperSquare(path)).retracted, 2)
  output = tableFromIPC(bytes(path))
  assert.deepEqual([...output.getChild('speed_taper')!], [0, 0, 0, 44])
  for (const field of retiredInput.schema.fields.filter(field => field.name !== 'speed_taper')) {
    assert.deepEqual([...output.getChild(field.name)!], [...retiredInput.getChild(field.name)!], field.name)
  }
  assert.deepEqual([...output.schema.metadata], [...input.schema.metadata])
})

test('non-CZ squares preserve bytes and counts while still validating country and taper columns', async () => {
  const path = writeRoadsFixture('foreign-speed-only.arrow', [4, 4], {
    speeds: [100, 0], countryCodes: [iso2Code('AT'), 0],
  })
  await withArrowWrite(path, table => {
    const columns = Object.fromEntries(table.schema.fields.map(field => [field.name, table.getChild(field.name)!]))
    columns.segment_idx = vectorFromArray([0, 0], new Int16())
    columns.length_m = vectorFromArray([60, 60], new Float32())
    for (const name of ['access', 'junction', 'built_up']) columns[name] = vectorFromArray([0, 0], new Uint8())
    for (const name of ['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto']) {
      columns[name] = vectorFromArray([123.125, 456.75], new Float64())
    }
    columns.speed_taper = vectorFromArray([44, 77], new Uint8())
    return new Table(columns)
  })
  const input = bytes(path), modified = statSync(path).mtimeMs
  assert.deepEqual(await enrichTaperSquare(path), {
    rows: 2, matched: 0, retracted: 0, updated: false, boundaries: 0, foreignRows: 2,
  })
  assert.deepEqual(bytes(path), input)
  assert.equal(statSync(path).mtimeMs, modified)

  await withArrowWrite(path, table => new Table({
    ...Object.fromEntries(table.schema.fields.map(field => [field.name, table.getChild(field.name)!])),
    speed_taper: vectorFromArray([44, 77], new Uint16()),
  }))
  const malformedSpeed = bytes(path)
  await assert.rejects(enrichTaperSquare(path), /invalid speed_taper column/)
  assert.deepEqual(bytes(path), malformedSpeed)
  await withArrowWrite(path, table => new Table({
    ...Object.fromEntries(table.schema.fields.map(field => [field.name, table.getChild(field.name)!])),
    country_iso: vectorFromArray([0, 0], new Uint8()),
  }))
  const malformedCountry = bytes(path)
  await assert.rejects(enrichTaperSquare(path), /country_iso.*Uint16/)
  assert.deepEqual(bytes(path), malformedCountry)
})
