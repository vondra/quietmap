/** Thailand DRR class mapping and DOH fallback precedence tests. */

import assert from 'node:assert/strict'
import { test } from 'node:test'
import { matchThailandRoad } from './enrich-roads-th.js'
import { parseThailandDrrSource, thailandDrrTraffic } from './lib/roads-th-source.js'
import type { RoadRow } from './lib/roads-arrow.js'

const HEADER = 'road_code,sum_AADT,MC,SV,SVT,TB2,TB3,T4,ART3,ART4,ART5,ART6,BD,DRT'
const road = (ref: string, latitude = 15, longitude = 101): RoadRow => ({
  startLat: latitude, startLon: longitude, endLat: latitude, endLon: longitude,
  midLat: latitude, midLon: longitude, roadClass: 2, ref, name: null, osmId: 1, existingSourceId: 0,
})

test('DRR parser maps buses to medium, articulated trucks to heavy and rejects unusable rows', () => {
  const source = parseThailandDrrSource([
    HEADER,
    'นบ.3021,100,20,30,2,5,6,7,8,9,10,1,3,4',
    'blank,100,,,,,,,,,,,,',
    'bad,100,nope,1,0,0,0,0,0,0,0,0,0,0',
    ',100,1,1,1,1,1,1,1,1,1,1,1,1',
  ].join('\n'))
  assert.deepEqual({ rows: source.sourceRows, records: source.records.size,
    unavailable: source.unavailableTrafficSkipped, invalid: source.invalidClassCountsSkipped },
  { rows: 4, records: 1, unavailable: 1, invalid: 2 })
  assert.deepEqual(thailandDrrTraffic(source.records.get('นบ.3021')!),
    { light: 32, medium: 12, heavy: 41, moto: 20 })
})

test('DRR exact ref wins before multi-ref motorway and Bangkok trunk policy', () => {
  const source = parseThailandDrrSource(`${HEADER}\n7;9,10,1,2,0,1,1,0,0,0,0,0,0,0\n`)
  assert.deepEqual(matchThailandRoad(road('7;9'), source),
    { kind: 'drr', light: 2, medium: 1, heavy: 1, moto: 1 })
  assert.deepEqual(matchThailandRoad(road('x; 9'), source),
    { kind: 'motorway', light: 62000, medium: 10000, heavy: 13000, moto: 15000 })
  assert.deepEqual(matchThailandRoad(road('35', 13.8, 100.5), source),
    { kind: 'trunk', light: 78000, medium: 10400, heavy: 9100, moto: 32500 })
  assert.equal(matchThailandRoad(road('unknown'), source), null)
})
