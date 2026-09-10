/** Source admission regressions for complete special-national inputs and ordered concession holes. */

import assert from 'node:assert/strict'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { resolve } from 'node:path'
import { test } from 'node:test'
import { GEM_COUNTRIES, gemAreaContains } from './industrial-gem-countries.js'
import { SPECIAL_COUNTRIES } from './industrial-special-countries.js'
import { SPECIAL_FEEDS } from './industrial-special-policy.js'
import { classifySpecialPoints, readSpecialFeatures, type SpecialFeature } from './industrial-special-source.js'
import { colombianConcessions, concessionClassifier, nationalContainmentAreas } from './industrial-special-polygons.js'

const point = (lat: number, lon: number, properties: Record<string, unknown>): SpecialFeature => ({ geometry: { type: 'Point', coordinates: [lon, lat] }, properties })

test('load-bearing sources distinguish missing, malformed and retired feeds; original unknown-geometry exclusions remain accounted', () => {
  const work = mkdtempSync(resolve(tmpdir(), 'special-admission-'))
  try {
    const path = resolve(work, 'source.geojson')
    assert.throws(() => readSpecialFeatures(path), /ENOENT/)
    writeFileSync(path, JSON.stringify({ type: 'FeatureCollection', features: [] }))
    assert.throws(() => readSpecialFeatures(path), /nonempty/)
    const id = SPECIAL_COUNTRIES.find(p => p.country === 'ID')!, feed = SPECIAL_FEEDS.ID[0]
    const retired = [point(-7, 110, { Status: 'retired', Fuel: 'coal' }), { geometry: null }]
    writeFileSync(path, JSON.stringify({ type: 'FeatureCollection', features: retired }))
    const result = classifySpecialPoints(readSpecialFeatures(path).features, id, feed, new Set())
    assert.equal(result.counts.unlocated, 1); assert.equal(result.counts.inactive, 1)
    assert.deepEqual(result.facilities, [], 'valid retired source retains authority for the country sweep')
    assert.throws(() => classifySpecialPoints([point(-7, Number.NaN, {})], id, feed, new Set()), /invalid point/)
    const cn = SPECIAL_COUNTRIES.find(p => p.country === 'CN')!
    assert.throws(() => classifySpecialPoints([point(31, 112, { Status: 'retired' })], cn, SPECIAL_FEEDS.CN[0], new Set()), /empty active/)
  } finally { rmSync(work, { recursive: true, force: true }) }
})

test('original cross-feed dedup order retains an unclassified first observation and exact special status/fuel policies', () => {
  const bo = SPECIAL_COUNTRIES.find(p => p.country === 'BO')!, seen = new Set<string>()
  assert.equal(classifySpecialPoints([point(-17, -64, { Tipo: 'EO' })], bo, SPECIAL_FEEDS.BO[0], seen).facilities.length, 0)
  const second = classifySpecialPoints([point(-17, -64, { Tipo: 'TG' })], bo, SPECIAL_FEEDS.BO[1], seen)
  assert.equal(second.counts.duplicate, 1); assert.equal(second.facilities.length, 0)
  const cases: Array<[string, number, Record<string, unknown>, number | null, boolean]> = [
    ['BR', 0, { ESTAGIO: 'Operação' }, 3511, true], ['CN', 4, { Status: '运营中' }, 3599, true],
    ['CO', 0, { Status: 'operating', Type: 'wind' }, null, true],
    ['VE', 1, { 'OPERACIÓN_ACTUAL_MW': '0' }, 3511, false],
    ['VE', 2, { Status: 'operating', Type: 'pumped storage' }, 3512, true],
    ['ZA', 0, { CATEGORY: 'CSP' }, 3599, true], ['ZA', 2, { Status: 'operating' }, 500, false],
    ['ID', 0, { Fuel: 'geothermal' }, 3511, true], ['VN', 0, { Type: 'solar' }, 3599, true],
  ]
  for (const [country, index, properties, nace, active] of cases) {
    const f = SPECIAL_FEEDS[country][index]
    assert.equal(f.classify(properties), nace, country)
    assert.equal(f.active?.(properties) ?? true, active, country)
  }
  const fj = SPECIAL_COUNTRIES.find(p => p.country === 'FJ')!
  for (const lon of [-179, 179]) assert.equal(gemAreaContains(fj, -17, lon, 0), true)
  assert.equal(gemAreaContains(fj, -17, 0, 0), false, 'same two-half mask controls sources and prepared rows')
})

test('Colombian final tier respects holes and source order rather than assigning a mine to its excluded courtyard', () => {
  const outer = [[-75,4],[-73,4],[-73,6],[-75,6],[-75,4]]
  const hole = [[-74.5,4.5],[-73.5,4.5],[-73.5,5.5],[-74.5,5.5],[-74.5,4.5]]
  const features: SpecialFeature[] = [{ geometry: { type: 'Polygon', coordinates: [outer, hole] }, properties: { ETAPA: 'Explotación', MINERALES: 'CARBÓN' } },
    { geometry: null }, { geometry: { type: 'Polygon', coordinates: [outer] }, properties: { ETAPA: 'Exploración', MINERALES: 'ORO' } }]
  const admitted = colombianConcessions(features, true, [-4.3,-82,13.5,-66.8])
  assert.equal(admitted.counts.unlocated, 1); assert.equal(admitted.counts.inactive, 1)
  const classify = concessionClassifier(admitted.polygons)
  assert.equal(classify(5, -74), null); assert.equal(classify(4.2, -74), 500)
  const oil = colombianConcessions([{ geometry: { type: 'Polygon', coordinates: [outer] }, properties: { ESTAD_AREA: 'EN PRODUCCION' } }], false, [-4.3,-82,13.5,-66.8])
  const combined = concessionClassifier([...admitted.polygons, ...oil.polygons])
  assert.equal(combined(5, -74), 600); assert.equal(combined(4.2, -74), 500)
  assert.throws(() => colombianConcessions([features[1]], true, [-4.3,-82,13.5,-66.8]), /empty admitted/)
})


test('four retained national families keep their source geometry, status, sector and containment semantics', () => {
  assert.equal(SPECIAL_COUNTRIES.length, 14)
  const specialCodes = SPECIAL_COUNTRIES.map(policy => policy.country).sort()
  assert.deepEqual(Object.keys(SPECIAL_FEEDS).sort(), specialCodes)
  assert.deepEqual(GEM_COUNTRIES.filter(policy => specialCodes.includes(policy.country)), [])
  const country = (iso: string) => SPECIAL_COUNTRIES.find(policy => policy.country === iso)!
  const clSeen = new Set<string>()
  const clThermal = classifySpecialPoints([point(-30, -70, { ESTADO: 'OPERATIVA' })],
    country('CL'), SPECIAL_FEEDS.CL[0], clSeen)
  const clDuplicate = classifySpecialPoints([point(-30, -70, { Status: 'operating', Type: 'solar' })],
    country('CL'), SPECIAL_FEEDS.CL[1], clSeen)
  assert.equal(clThermal.facilities[0].nace4, 3511)
  assert.equal(clDuplicate.counts.duplicate, 1)

  const powerCases: Array<[string, number, Record<string, unknown>, number | null, boolean]> = [
    ['CL', 1, { Status: 'operating', Type: 'solar' }, 3599, true],
    ['CL', 1, { Status: 'operating', Type: 'hydropower' }, 3512, true],
    ['CL', 1, { Status: 'operating', Type: 'wind' }, null, true],
    ['CL', 1, { Status: 'retired', Type: 'coal' }, 3511, false],
    ['PE', 0, { Status: 'operating', Type: 'solar' }, 3599, true],
    ['PE', 0, { Status: 'operating', Type: 'wind' }, null, true],
    ['PH', 0, { Status: 'operating', FuelType: 'hydropower' }, 3512, true],
    ['PH', 0, { Status: 'retired', FuelType: 'coal' }, 3511, false],
    ['IN', 1, { primary_fuel: 'Solar' }, 3599, true],
    ['IN', 1, { primary_fuel: 'Hydro' }, 3512, true],
    ['IN', 1, { primary_fuel: 'Wind' }, null, true],
    ['IN', 1, { primary_fuel: 'Coal' }, 3511, true],
  ]
  for (const [iso, index, properties, nace, active] of powerCases) {
    const feed = SPECIAL_FEEDS[iso][index]
    assert.equal(feed.classify(properties), nace, `${iso} fuel`)
    assert.equal(feed.active?.(properties) ?? true, active, `${iso} status`)
  }

  const indiaPark = classifySpecialPoints([point(20, 78, { pollution_cat: 'Orange' })],
    country('IN'), SPECIAL_FEEDS.IN[2], new Set())
  assert.equal(indiaPark.facilities[0].nace4, 2000)
  const cement = classifySpecialPoints([{
    geometry: { type: 'Polygon', coordinates: [[[77, 20], [79, 20], [79, 22], [77, 22], [77, 20]]] },
    properties: {},
  }], country('IN'), SPECIAL_FEEDS.IN[0], new Set())
  assert.deepEqual([cement.facilities[0].lat, cement.facilities[0].lon, cement.facilities[0].nace4], [21, 78, 2300])

  const peru = country('PE'), outer = [[-76,-12],[-74,-12],[-74,-10],[-76,-10],[-76,-12]]
  const hole = [[-75.5,-11.5],[-74.5,-11.5],[-74.5,-10.5],[-75.5,-10.5],[-75.5,-11.5]]
  const areas = nationalContainmentAreas([{
    geometry: { type: 'Polygon', coordinates: [outer, hole] }, properties: {},
  }], { file: 'mine', nace4: 700 }, peru.bbox)
  const classify = concessionClassifier(areas.polygons)
  assert.equal(classify(-11, -75), null)
  assert.equal(classify(-11.8, -75), 700)

  const philippines = classifySpecialPoints([point(14, 121, { Status: 'operating', Type: 'coal' })],
    country('PH'), SPECIAL_FEEDS.PH[0], new Set())
  assert.equal(philippines.facilities[0].nace4, 3511)
})
