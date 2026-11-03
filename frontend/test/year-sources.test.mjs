// Public metadata and release visibility cannot remove methods or leak acquisition details.
import assert from 'node:assert/strict'
import test from 'node:test'
import { existsSync, readFileSync } from 'node:fs'
import { readYears, renderYearSources, validatePublicManifest, REGIONS, PRIVATE_SOURCE_TEXT } from '../scripts/year-sources.ts'

const source = (id, region, public_ok = true) => ({ id, kind: 'prepare-road-counts', region, public_ok,
  publisher: id, url: 'https://example.org/counts', observed: { start: '1999', end: '2000' }, published: '2002', licence: 'CC0',
  depends_on: [], description: 'Counted traffic.' })
const manifest = () => ({ year: '2000', built: true, as_of: '2001-01-01', flows: { start: '2000-01-01', end: '2001-01-01' },
  inputs: [source('Local', 'CZ'), source('City', 'CZ:Prague'), source('Regional', 'europe'), source('Global', 'world'), source('Restricted records', 'CZ', false)], unavailable: [] })

test('local citations show observation and publication separately; common lists live once', () => {
  const m = manifest()
  const years = [{ name: m.year, manifest: m }]
  const text = renderYearSources('data-sources/europe/cz.md', years)
  for (const label of ['### 2000', 'Local', 'City', 'Restricted records', '1999 to 2000', 'Published 2002']) assert.ok(text.includes(label), label)
  for (const label of ['[Global]', '[Regional]']) assert.ok(!text.includes(label))
  assert.ok(renderYearSources('data-sources/europe/index.md', years).includes('[Regional]'))
  assert.ok(renderYearSources('data-sources.md', years).includes('[Global]'))
  m.built = false
  assert.ok(!renderYearSources('data-sources/europe/cz.md', years).includes('### 2000'))
})

test('the public projection rejects private fields, leaked phrases, moving URLs and unknown kinds', () => {
  for (const leaked of ['private enrichment inventory', 'example-cache', 'paged storage', '/private/storage', '[latest]', 'table{01..47}.csv']) {
    const m = manifest(); m.inputs[0].description = leaked
    assert.throws(() => validatePublicManifest(m), /Private|unpinned/)
  }
  for (const [key, value] of [['path', 'somewhere'], ['kind', 'prepare-typo'], ['region', 'Atlantis'], ['url', 'https://example.org/latest.zip']]) {
    const m = manifest(); m.inputs[0][key] = value; assert.throws(() => validatePublicManifest(m))
  }
})

test('all real source regions have pages; built maps alone are published with their methods', () => {
  const years = readYears()
  for (const path of Object.values(REGIONS)) {
    assert.ok(existsSync(new URL(`../src/about/${path}.md`, import.meta.url)) || existsSync(new URL(`../src/about/${path}/index.md`, import.meta.url)), path)
  }
  for (const [page, method] of [['data-sources/asia/th.md', 'vehicle-kilometres'], ['data-sources/north-america/us.md', '## Roads']]) {
    const markdown = readFileSync(new URL(`../src/about/${page}`, import.meta.url), 'utf8')
    assert.ok(markdown.includes(method), page)
    const text = renderYearSources(page, years)
    for (const year of years) assert.equal(text.includes(`### ${year.name}`), year.manifest.built)
    assert.ok(!PRIVATE_SOURCE_TEXT.test(text))
  }
})

test('popup citations change with the same manifest and never borrow a different year', async () => {
  const { datasetCitations } = await import('../scripts/dataset-citations.ts')
  const years = readYears()
  const current = years.find(year => year.manifest.built).manifest
  const copy = structuredClone(current)
  const input = copy.inputs.find(input => input.kind === 'road-reference-counts')
  input.publisher = 'Synthetic traffic survey'
  input.observed = { start: '1990', end: '1991' }
  input.url = 'https://example.org/edition-1991'
  assert.deepEqual(datasetCitations(copy)[31], ['Synthetic traffic survey', '1990 to 1991', 'counted', 'https://example.org/edition-1991'])
  copy.inputs = []
  assert.equal(datasetCitations(copy)[31], undefined)
  assert.deepEqual(datasetCitations(copy)[9012], ['Road-class model', '', 'proxy', ''])
})


test('the preparation page takes the national road priors and aircraft window from their inputs', async () => {
  const { renderClassPriors, renderAircraftWindows } = await import('../scripts/year-sources.ts')
  const years = readYears()
  const table = renderClassPriors(years)
  assert.ok(table.includes('| Czechia | Tertiary | 632 | 415 | 962 |'))
  const m = manifest(); m.flows.increment_days = ['2000-01-01', '2000-03-01']; m.flows.excluded_days = ['2000-02-01']
  const text = renderAircraftWindows([{ name: m.year, manifest: m }])
  assert.ok(text.includes('2000-01-01 to 2001-01-01') && text.includes('2 sample days') && text.includes('1 missing day): 2000-02-01'))
})


test('a city collection credits its common publisher once and model codes invent no observation year', async () => {
  const { datasetCitations } = await import('../scripts/dataset-citations.ts')
  const m = manifest()
  m.inputs = [source('eu-city-brno', 'CZ:Brno'), source('eu-city-vienna', 'AT:Vienna')]
  m.inputs[0].publisher = 'Brno; Shared traffic survey'
  m.inputs[1].publisher = 'Vienna; Shared traffic survey'
  m.inputs[0].observed = { start: '1990', end: '1991' }
  m.inputs[1].observed = { start: '1999', end: '1999' }
  const citations = datasetCitations(m)
  assert.deepEqual(citations[10], ['Shared traffic survey', '1990 to 1999', 'counted', 'https://example.org/counts'])
  assert.deepEqual(citations[9860], ['Road-class model', '', 'proxy', ''])
})


test('every established popup provenance code retains its citation', async () => {
  const { datasetCitations } = await import('../scripts/dataset-citations.ts')
  const current = readYears().find(year => year.manifest.built).manifest
  const citations = datasetCitations(current)
  const codes = [10, 11, 12, 20, 21, 22, 23, 24, 30, 31, 100, 110, 111, 112, 1004, 1013, 1019, 1023, 1025, 1026, 1031, 1034, 1037, 1039, 1041, 1048, 1049, 1050, 1054, 1088, 1090, 1093, 1095, 1097, 1098, 1102, 1103, 1113, 1124, 2000, 2004, 2005, 2009, 2015, 2021, 2024, 2028, 2030, 2035, 2036, 2037, 2040, 2044, 2057, 2066, 2067, 2073, 2075, 9003, 9004, 9005, 9012, 9013, 9180, 9181, 9231, 9232, 9263, 9364, 9365, 9368, 9369, 9392, 9398, 9399, 9404, 9405, 9484, 9504, 9505, 9528, 9566, 9567, 9643, 9644, 9705, 9729, 9730, 9752, 9756, 9792, 9793, 9804, 9805, 9818, 9819, 9834, 9835, 9860, 9861, 9862, 9863, 9864, 9865, 9870, 9871, 9872, 9873, 9874, 9875, 9876]
  for (const code of codes) assert.ok(citations[code]?.[0], `Missing citation for ${code}`)
  assert.ok(citations[10][0].length < 100)
  for (const code of [2024, 2028, 2030, 2035, 2057, 2073, 2075]) assert.ok(citations[code][1])
})
