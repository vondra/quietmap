/** GB DfT loader tests: row selection, slip-road points, class gates, minor-road location matching and baked ownership. */

import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { tableFromIPC } from 'apache-arrow'
import { assignMinorPointsToWays, enrichGreatBritainRoads, majorRoadIndex, matchDftPoint } from './enrich-roads-gb.js'
import { selectDftCountPoints, type DftCountPoint } from './lib/roads-gb-source.js'
import { EXCLUDE_HOLDOUT_COUNTS_ENVIRONMENT } from './lib/count-holdout.js'
import { iso2Code } from './lib/prepared-grid.js'
import { writeRoadsFixture } from './lib/road-test-fixture.js'
import type { RoadRow } from './lib/roads-arrow.js'

const TEST_DIRECTORY = mkdtempSync(join(tmpdir(), 'enrich-roads-gb-test-'))
after(() => rmSync(TEST_DIRECTORY, { recursive: true, force: true }))

const csvRow = (values: Record<string, string | number>): Record<string, string> => ({
  road_category: 'PA', start_junction_road_name: '', end_junction_road_name: '', link_length_km: '5',
  estimation_method: 'Counted', latitude: '51.5', longitude: '-0.1',
  cars_and_taxis: '200', LGVs: '20', buses_and_coaches: '8', all_HGVs: '40', two_wheeled_motor_vehicles: '4', all_motor_vehicles: '272',
  ...Object.fromEntries(Object.entries(values).map(([key, value]) => [key, String(value)])),
})

function road(overrides: Partial<RoadRow> = {}): RoadRow {
  return {
    startLat: 51.5, startLon: -0.1, endLat: 51.51, endLon: -0.09,
    midLat: 51.505, midLon: -0.095, ref: 'A1', name: null,
    osmId: 1, roadClass: 1, existingSourceId: 0, ...overrides,
  }
}

function point(overrides: Partial<DftCountPoint> = {}): DftCountPoint {
  return {
    countBasis: 'both-directions', observationId: '1',
    ref: 'A1', latitude: 51.505, longitude: -0.095, roadCategory: 'PA', rank: 2, isRamp: false,
    light: 1000, medium: 40, heavy: 100, moto: 10, total: 1150, year: 2024,
    startJunction: '', endJunction: '', linkLengthKm: 5,
    ...overrides,
  }
}

test('DfT selection keeps the latest major row, the latest manual minor count and valid class splits only', async () => {
  const points = await selectDftCountPoints([
    csvRow({ count_point_id: 1, year: 2023, road_name: 'A 1', all_motor_vehicles: 272 }),
    csvRow({ count_point_id: 1, year: 2025, road_name: 'A 1', estimation_method: 'Estimated' }),
    csvRow({ count_point_id: 2, year: 2014, road_name: 'A2' }),
    csvRow({ count_point_id: 3, year: 2025, road_name: 'A3', all_motor_vehicles: 270 }), // classes exceed total by 2: rounding
    csvRow({ count_point_id: 4, year: 2025, road_name: 'A4', all_motor_vehicles: 269 }), // by 3: impossible
    csvRow({ count_point_id: 5, year: 2024, road_name: 'U', road_category: 'MCU' }),
    csvRow({ count_point_id: 5, year: 2025, road_name: 'U', road_category: 'MCU', estimation_method: 'Estimated' }),
    csvRow({ count_point_id: 6, year: 2020, road_name: 'C', road_category: 'MCU' }),
  ])
  assert.deepEqual(points.map(({ observationId, ref, year, rank }) => [observationId, ref, year, rank]),
    [['1', 'A1', 2025, 2], ['3', 'A3', 2025, 2], ['5', 'U', 2024, 5]])
})

test('slip-road points by junction name or by a share of their own road never stamp a mainline', async () => {
  // M1 J13 (DfT 2024): the slip links 81571 and 93199 beside the mainline point 81548 (96,385/day).
  const m1 = { road_name: 'M1', road_category: 'TM', year: 2024 }
  const points = await selectDftCountPoints([
    csvRow({ ...m1, count_point_id: 81548, link_length_km: 4.2, latitude: 52.02, longitude: -0.6, all_motor_vehicles: 96385,
      cars_and_taxis: 70000, LGVs: 16000, all_HGVs: 10000, buses_and_coaches: 200, two_wheeled_motor_vehicles: 185 }),
    csvRow({ ...m1, count_point_id: 81571, link_length_km: 0.6, latitude: 52.021, longitude: -0.601,
      start_junction_road_name: 'M1 J13 off slip', end_junction_road_name: 'A507' }),
    csvRow({ ...m1, count_point_id: 93199, link_length_km: 0.8, latitude: 52.019, longitude: -0.599, all_motor_vehicles: 3121,
      cars_and_taxis: 2500, LGVs: 400, all_HGVs: 200, buses_and_coaches: 10, two_wheeled_motor_vehicles: 11 }),
  ])
  assert.deepEqual(points.map(({ observationId, isRamp }) => [observationId, isRamp]), [['81548', false], ['81571', true], ['93199', true]])
  const matched = matchDftPoint(road({ ref: 'M1', roadClass: 0, midLat: 52.021, midLon: -0.601 }), majorRoadIndex(points))
  assert.equal(matched?.observationId, '81548')
})

test('a ref match within 15 km stamps only a road of the class DfT counted, never a slip road', () => {
  const near = point()
  const index = majorRoadIndex([point({ latitude: 54 }), near])
  assert.equal(matchDftPoint(road({ ref: ' A 1 ' }), index), near)
  assert.equal(matchDftPoint(road({ ref: 'A2' }), index), null)
  assert.equal(matchDftPoint(road({ midLat: 50 }), index), null)
  assert.equal(matchDftPoint(road({ roadClass: 10 }), index), null, 'a slip road never takes the mainline count')
  assert.equal(matchDftPoint(road({ roadClass: 8 }), index), null, 'a track tagged A1 takes no principal A-road count')
  assert.equal(matchDftPoint(road({ roadClass: 5 }), index), null, 'a residential street takes no principal A-road count')
})

test('a minor-road count stamps the whole way it sits on, unless another class runs within 20 m', () => {
  const prepared = join(TEST_DIRECTORY, 'minor')
  // Two residential ways; a tertiary road in the next square overlaps the second one exactly.
  for (const [square, classes, origin] of [['z9/255/170', [5, 5], [14, 50]], ['z9/255/171', [4], [14.001, 50.001]]] as const) {
    mkdirSync(join(prepared, square), { recursive: true })
    copyFileSync(writeRoadsFixture(`minor-${classes.length}.arrow`, [...classes], { origin: [...origin],
      countryCodes: classes.map(() => iso2Code('GB')) }), join(prepared, square, 'roads.arrow'))
  }
  const minor = (id: string, latitude: number, longitude: number) => point({ observationId: id, ref: 'U', roadCategory: 'MCU', rank: 5,
    latitude, longitude, linkLengthKm: null })
  const ways = assignMinorPointsToWays(prepared, [minor('on-way', 50.00005, 14.00004), minor('beside-tertiary', 50.00105, 14.00104),
    minor('off-way', 50.0003, 14.0001)])
  assert.deepEqual([...ways].map(([way, points]) => [way, points.map(each => each.observationId)]), [[10_000, ['on-way']]])
})

test('z9 GB pass writes domestic data and heals a matching foreign GB stamp', async () => {
  const prepared = join(TEST_DIRECTORY, 'prepared')
  const square = join(prepared, 'z9', '255', '170')
  mkdirSync(square, { recursive: true })
  const source = writeRoadsFixture('gb-loader.arrow', [1, 1], {
    refs: ['A1', 'A1'],
    countryCodes: [iso2Code('GB'), iso2Code('IE')],
    sourceIds: [0, 1041],
  })
  const target = join(square, 'roads.arrow')
  copyFileSync(source, target)
  const measured = point({ latitude: 50.00025, longitude: 14.00025 })

  const result = await enrichGreatBritainRoads(prepared, [measured])
  assert.deepEqual(
    { matched: result.matched, retracted: result.retracted, skippedForeign: result.skippedForeign },
    { matched: 1, retracted: 1, skippedForeign: 1 },
  )
  const table = tableFromIPC(readFileSync(target))
  assert.deepEqual([...Array(2)].map((_, index) => table.getChild('source_id')!.get(index)), [1041, 0])
  assert.equal(table.getChild('aadt_light')!.get(0), 1000)
  assert.equal(table.schema.metadata.get('roads_contract'), 'country_baked_v1')
})

test('a physical holdout DfT point cannot stamp an adjacent training square', async () => {
  // The replay put point 805005 (holdout 246/154) on B887 way 24931663 in training 245/154.
  const rows = [csvRow({ count_point_id: 805005, year: 2025, road_name: 'B887', road_category: 'MB',
    latitude: 57.96539637, longitude: -7.00519807 })]
  const target = road({ ref: 'B887', roadClass: 3, midLat: 57.9654, midLon: -7.04 })
  const normal = await selectDftCountPoints(rows)
  assert.equal(matchDftPoint(target, majorRoadIndex(normal))?.observationId, '805005')
  process.env[EXCLUDE_HOLDOUT_COUNTS_ENVIRONMENT] = '1'
  try {
    const withheld = await selectDftCountPoints(rows)
    assert.equal(matchDftPoint(target, majorRoadIndex(withheld)), null)
  } finally {
    delete process.env[EXCLUDE_HOLDOUT_COUNTS_ENVIRONMENT]
  }
})
