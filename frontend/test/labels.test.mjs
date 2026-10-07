// A contributor's label: its name; a road or line known only by its number, with what it numbers;
// else its class. What the aircraft layer is made of, largest kind first.
import assert from 'node:assert/strict'
import test from 'node:test'

import { aircraftKindShares, aircraftMakeup, contributorLabel } from '../src/components/noise/labels.ts'

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

test('the aircraft layer reads as its largest kind, its makeup largest first', () => {
  assert.equal(aircraftMakeup({ airliners: 0.95, propeller: 0.03, regional_business_jets: 0.02 }), 'airliners')
  assert.equal(aircraftMakeup({ airliners: 0.35, helicopters: 0.40, propeller: 0.25 }), 'helicopters')
  assert.equal(aircraftMakeup(undefined), null)
  assert.deepEqual(aircraftKindShares({ propeller: 0.09, airliners: 0.78, helicopters: 0.13 }),
    [['Airliners', 0.78], ['Helicopters', 0.13], ['Propeller aircraft', 0.09]])
})
