import assert from 'node:assert/strict'
import test from 'node:test'

import {
  EUROPE_VIEW,
  fetchIpCountry,
  resolveInitialView,
} from '../src/utils/initial-view.ts'

// --- resolveInitialView: the priority lattice ---
// IP country → browser-language region subtag → unambiguous language → Europe.

test('IP country wins over the browser-language region', () => {
  // en-US region would give the US view, but the IP says CZ — IP wins.
  assert.deepEqual(resolveInitialView(['en-US'], 'CZ'), { lat: 49.8, lng: 15.5, zoom: 7 })
})

test('a newly-curated European country resolves from its IP code', () => {
  assert.deepEqual(resolveInitialView([], 'UA'), { lat: 48.8, lng: 31.3, zoom: 5 })
})

test('an IP country with no COUNTRY_VIEW entry falls through to language', () => {
  // JP is intentionally not curated (Europe-first) → the de-DE region decides.
  assert.deepEqual(resolveInitialView(['de-DE'], 'JP'), { lat: 51.2, lng: 10.4, zoom: 6 })
})

test('an uncovered IP country with no useful language defaults to Europe', () => {
  assert.deepEqual(resolveInitialView(['ja'], 'JP'), EUROPE_VIEW)
})

test('language region subtag resolves when there is no IP country', () => {
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

// --- fetchIpCountry: parsing + normalisation of the server contract ---

function withFetch(impl, run) {
  const original = globalThis.fetch
  globalThis.fetch = impl
  return Promise.resolve(run()).finally(() => {
    globalThis.fetch = original
  })
}

const jsonResponse = (body, ok = true) => ({ ok, json: async () => body })

test('fetchIpCountry returns the upper-cased ISO code on an ip-country body', () =>
  withFetch(
    async () => jsonResponse({ source: 'ip-country', country: 'cz' }),
    async () => assert.equal(await fetchIpCountry(500), 'CZ'),
  ))

test('fetchIpCountry returns null for source:none', () =>
  withFetch(
    async () => jsonResponse({ source: 'none' }),
    async () => assert.equal(await fetchIpCountry(500), null),
  ))

test('fetchIpCountry returns null on a non-ok response', () =>
  withFetch(
    async () => jsonResponse({ source: 'ip-country', country: 'CZ' }, false),
    async () => assert.equal(await fetchIpCountry(500), null),
  ))

test('fetchIpCountry returns null on a malformed country code', () =>
  withFetch(
    async () => jsonResponse({ source: 'ip-country', country: 'Czechia' }),
    async () => assert.equal(await fetchIpCountry(500), null),
  ))

test('fetchIpCountry returns null when fetch throws', () =>
  withFetch(
    async () => {
      throw new Error('network down')
    },
    async () => assert.equal(await fetchIpCountry(500), null),
  ))
