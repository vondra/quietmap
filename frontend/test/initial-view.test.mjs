// The first-visit view from the browser languages.
import assert from 'node:assert/strict'
import test from 'node:test'

import { EUROPE_VIEW, resolveInitialView } from '../src/utils/initial-view.ts'

// The priority lattice: browser-language region subtag → unambiguous language → Europe.

test('an uncovered language region with no useful language defaults to Europe', () => {
  assert.deepEqual(resolveInitialView(['ja-JP']), EUROPE_VIEW)
})

test('the first language with a known country decides', () => {
  assert.deepEqual(resolveInitialView(['ja-JP', 'de-DE']), { lat: 51.2, lng: 10.4, zoom: 6 })
})

test('a language region subtag resolves its country', () => {
  assert.deepEqual(resolveInitialView(['en-GB']), { lat: 54.0, lng: -2.5, zoom: 6 })
})

test('an unambiguous single-country language resolves without a region', () => {
  assert.deepEqual(resolveInitialView(['cs']), { lat: 49.8, lng: 15.5, zoom: 7 })
  assert.deepEqual(resolveInitialView(['uk']), { lat: 48.8, lng: 31.3, zoom: 5 })
})

test('an ambiguous language with no region falls back to whole Europe', () => {
  // Plain `en` could be US/GB/IE/… — never guess a country from it.
  assert.deepEqual(resolveInitialView(['en']), EUROPE_VIEW)
  assert.deepEqual(resolveInitialView([]), EUROPE_VIEW)
})
