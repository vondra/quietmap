// A contributor's label: its name; a road or line known only by its number, with what it numbers;
// else its class, a place whose sound is its people named by them. What the aircraft layer is made
// of, largest kind first.
import assert from 'node:assert/strict'
import test from 'node:test'

import { aircraftKindShares, contributorLabel, labelNamesClass, subtypeLabel } from '../src/components/noise/labels.ts'

const road = (name, metadata) => ({
  id: 'r', source_type: 'road', name, subtype: 'tertiary', distance_m: 38,
  received_lden: 50, received: { ld: 48, le: 46, ln: 40 }, metadata,
})

test('a road known only by its number reads as a road with that number', () => {
  assert.equal(contributorLabel(road('2404', { road_class: 'tertiary', ref: '2404' })), 'Road 2404')
  assert.equal(contributorLabel(road('D1', { road_class: 'motorway', ref: 'D1' })), 'Road D1')
})

test('a named road reads as its name, even with a number', () => {
  assert.equal(contributorLabel(road('Šárecká', { road_class: 'tertiary', name: 'Šárecká', ref: '2404' })), 'Šárecká')
})

test('a road with neither name nor number reads as its class', () => {
  assert.equal(contributorLabel(road('tertiary', { road_class: 'tertiary' })), 'Tertiary road')
})

test('a line known only by its number keeps its class, and its detail does not repeat it', () => {
  const tram = { ...road('22', { rail_type: 'tram', ref: '22' }), source_type: 'railway', subtype: null }
  assert.equal(contributorLabel(tram), 'Tram 22')
  assert.equal(labelNamesClass(tram), true)
  assert.equal(labelNamesClass(road('2404', { road_class: 'tertiary', ref: '2404' })), false)
  const school = { ...road('School of Economics', { building_type: 'education', name: 'School of Economics' }), source_type: 'building', subtype: null }
  assert.equal(labelNamesClass(school), false)
})

test('a place whose sound is its people reads as the place alone; boats in plain words', () => {
  const unnamed = (type) => contributorLabel({
    id: 'b', source_type: 'building', name: type, subtype: null, distance_m: 58,
    received_lden: 40, received: { ld: 30, le: 38, ln: 32 }, metadata: { building_type: type, name: '' },
  })
  assert.equal(unnamed('people_bar'), 'Bar')
  assert.equal(unnamed('people_biergarten'), 'Beer garden')
  assert.equal(unnamed('tennis_court'), 'Tennis court')
  assert.equal(unnamed('playground'), 'Playground')
  assert.equal(unnamed('artificial_turf_pitch'), 'Artificial-turf pitch')
  assert.equal(subtypeLabel('ship', 'leisure_craft'), 'Leisure boats')
  assert.equal(subtypeLabel('road', 'track'), 'Track')
})

test('the aircraft makeup reads largest first', () => {
  assert.deepEqual(aircraftKindShares({ propeller: 0.09, airliners: 0.78, helicopters: 0.13 }),
    [['Airliners', 0.78], ['Helicopters', 0.13], ['Propeller aircraft', 0.09]])
})
