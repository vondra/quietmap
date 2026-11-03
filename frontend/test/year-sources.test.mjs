// The source page follows the manifest, scopes local inputs and never publishes paths or private records.
import assert from 'node:assert/strict'
import test from 'node:test'
import { readYears, renderYearSources } from '../scripts/year-sources.ts'

const source = (id, region, public_ok = true) => ({ id, kind: 'road-counts', region, public_ok,
  publisher: id, url: 'https://example.org/counts', vintage: '1999', licence: 'CC0',
  description: 'Counted traffic.', path: '/private/not-for-the-browser' })

test('country sources follow every admitted year and keep private inputs out of the page', () => {
  const manifest = { as_of: '2001-01-01', flows: { start: '2000-01-01', end: '2001-01-01' },
    inputs: [source('Local', 'CZ'), source('City', 'CZ:Prague'), source('Regional', 'europe'),
      source('Global', 'world'), source('Elsewhere', 'TH'), source('Private', 'CZ', false)],
    unavailable: [{ id: 'gap', kind: 'road-counts', region: 'CZ', reason: 'No earlier counts; traffic is modelled.' }] }
  const text = renderYearSources('data-sources/europe/cz.md', [{ name: '2000', manifest }])
  for (const label of ['## 2000', 'Local', 'City', 'Regional', 'Global', 'No earlier counts']) assert.ok(text.includes(label), label)
  for (const label of ['Private', 'Elsewhere', '/private/', 'not-for-the-browser']) assert.ok(!text.includes(label), label)
  const changed = structuredClone(manifest)
  changed.inputs[0].vintage = '1998'
  assert.ok(renderYearSources('data-sources/europe/cz.md', [{ name: '2000', manifest: changed }]).includes('1998'))
})

test('each real country page contains both year manifests and their local sources', () => {
  const years = readYears()
  for (const [page, country] of [['data-sources/asia/th.md', 'TH'], ['data-sources/north-america/us.md', 'US']]) {
    const text = renderYearSources(page, years)
    for (const year of years) {
      assert.ok(text.includes(`## ${year.name}`))
      for (const input of year.manifest.inputs.filter(input => input.region === country && input.public_ok)) {
        assert.ok(text.includes(input.publisher), input.id)
      }
    }
    assert.ok(!text.includes('enrichment-world/'))
  }
})
