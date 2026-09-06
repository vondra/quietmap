/**
 * Guard rails for the Cerema TMJA census parser — the three ways the two
 * shipped CSVs read wrong: a CRLF-mangled header name, the 2019 `ratio_PL`
 * encoding, and a section identity that conflates neighbouring sections.
 *
 * Run: `cd pipeline && npx tsx --test lib/fr-tmja-census.test.ts`
 */

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { parseTmjaFiles, type TmjaCsvFile } from './fr-tmja-census.js'

/** The 2019 release's column order; 2024 adds `cote` but the parser reads by
 *  name, so one shape covers both. */
const COLUMNS = [
  'dateReferentiel', 'route', 'longueur', 'prD', 'depPrD', 'concessionPrD', 'absD', 'cumulD', 'xD', 'yD', 'zD',
  'prF', 'depPrF', 'concessionPrF', 'absF', 'cumulF', 'xF', 'yF', 'zF',
  'anneeMesureTrafic', 'typeComptageTrafic', 'typeComptageTrafic_lib', 'TMJA', 'ratio_PL',
]

function csv(rows: Record<string, string>[], lineEnding = '\r\n', columns = COLUMNS): string {
  const body = rows.map((row) => columns.map((name) => row[name] ?? '').join(';'))
  return [columns.join(';'), ...body].join(lineEnding) + lineEnding
}

/** N20 south of Toulouse, verbatim from the 2019 release: TMJA 26,900 with
 *  `ratio_PL` 91 — nine-point-one percent heavy, not ninety-one. */
const N20_2019 = { route: 'N0020', cumulD: '0', cumulF: '1000', xD: '571000', yD: '6230000', xF: '571500', yF: '6230500', TMJA: '26900', ratio_PL: '91' }

/** Two neighbouring 2024 sections of A26 (cumul 132945-133038 and
 *  133038-133666, Lambert-93 verbatim) whose midpoints share the 0.01° cell
 *  (50.14, 3.13) yet carry different traffic. */
const A26_LOW = { route: 'A0026', cumulD: '132945', cumulF: '133038', xD: '709031,04', yD: '7005173,65', xF: '709085,04', yF: '7005129,15', TMJA: '18403', ratio_PL: '21,015' }
const A26_HIGH = { route: 'A0026', cumulD: '133038', cumulF: '133666', xD: '709085,04', yD: '7005129,15', xF: '709546,26', yF: '7004743,8', TMJA: '21382', ratio_PL: '22,716' }

function file(rows: Record<string, string>[], over: Partial<TmjaCsvFile> = {}): TmjaCsvFile {
  return { label: 'TMJA 2019', csvText: csv(rows), ratioPlEncoding: 'tenths-of-percent-when-integer', ...over }
}

test('a CRLF header still finds ratio_PL, and 2019 integers are tenths of a percent', () => {
  // Reading the header with split('\n')+split(';') leaves `ratio_PL\r` as the
  // last column name, so every section fell back to a flat 0.12 heavy share
  // and N20 was split [23403, 65, 3163, 269].
  const { sections } = parseTmjaFiles([file([N20_2019])])
  assert.equal(sections.length, 1)
  const s = sections[0]
  assert.equal(s.ref, 'N20')
  assert.equal(s.tmja, 26900)
  assert.equal(Math.round(s.ratio_pl * 1000), 91) // 9.1 %, not 91 %
  assert.deepEqual([s.aadt_light, s.aadt_medium, s.aadt_heavy, s.aadt_moto], [24183, 49, 2399, 269])
})

test('LF and CRLF inputs produce identical sections', () => {
  const rows = [N20_2019, A26_LOW]
  const crlf = parseTmjaFiles([file(rows, { csvText: csv(rows, '\r\n') })]).sections
  const lf = parseTmjaFiles([file(rows, { csvText: csv(rows, '\n') })]).sections
  assert.deepEqual(lf, crlf)
  assert.equal(crlf.length, 2)
})

test('2019 decimals stay percent while 2024 reads every value as percent', () => {
  const decimal2019 = parseTmjaFiles([file([{ ...N20_2019, ratio_PL: '17,01' }])]).sections[0]
  assert.equal(decimal2019.ratio_pl.toFixed(4), '0.1701')
  const percent2024 = parseTmjaFiles([file([{ ...N20_2019, ratio_PL: '91' }], { label: 'TMJA 2024', ratioPlEncoding: 'percent' })]).sections[0]
  assert.equal(percent2024.ratio_pl.toFixed(2), '0.91')
})

test('a zero, an empty and a vehicle-count ratio_PL all fall back to 0.12', () => {
  // The 2024 file carries one zero and six counts on A0043 (6,285-12,724);
  // 2019 carries seven counts. None of them is a heavy-vehicle share.
  for (const ratio_PL of ['0', '', '12724']) {
    const s = parseTmjaFiles([file([{ ...N20_2019, ratio_PL }])]).sections[0]
    assert.equal(s.ratio_pl, 0.12, `ratio_PL=${JSON.stringify(ratio_PL)}`)
  }
})

test('neighbouring sections of one route survive; the older release yields to the newer', () => {
  const newer: TmjaCsvFile = { label: 'TMJA 2024', csvText: csv([A26_LOW, A26_HIGH]), ratioPlEncoding: 'percent' }
  // Same two sections re-surveyed in 2019 with different traffic, plus one the
  // 2024 release does not cover at all.
  const older = file([
    { ...A26_LOW, TMJA: '15000' },
    { ...A26_HIGH, TMJA: '16000' },
    { ...A26_HIGH, cumulD: '133666', cumulF: '142369', TMJA: '17000' },
  ])
  const { sections, counters } = parseTmjaFiles([newer, older])
  assert.deepEqual(sections.map((s) => s.tmja), [18403, 21382, 17000])
  assert.equal(counters[0].skippedDuplicateSection, 0)
  assert.equal(counters[1].skippedDuplicateSection, 2)
})

test('a missing column throws instead of reading as a missing measurement', () => {
  const withoutRatio = COLUMNS.filter((name) => name !== 'ratio_PL')
  const csvText = csv([N20_2019], '\r\n', withoutRatio)
  assert.throws(() => parseTmjaFiles([file([], { csvText })]), /missing column\(s\) ratio_PL/)
})

test('a percent-encoded share above 100 % is a count, not a share, and falls back', () => {
  // Six 2024 rows on A0043 carry vehicle counts in ratio_PL; one under the raw
  // cap of 500 would otherwise write a negative light-vehicle class.
  const { sections } = parseTmjaFiles([file([{ ...N20_2019, ratio_PL: '150' }],
    { label: 'TMJA 2024', ratioPlEncoding: 'percent' })])
  assert.equal(sections.length, 1)
  assert.equal(sections[0].ratio_pl, 0.12)
  assert.ok(sections[0].aadt_light > 0)
})

test('the 2019 tenths rule reads the written form, so "12,0" is twelve percent', () => {
  const [decimalForm, integerForm] = parseTmjaFiles([file([
    { ...N20_2019, cumulD: '0', ratio_PL: '12,0' },
    { ...N20_2019, cumulD: '2000', cumulF: '3000', ratio_PL: '12' },
  ])]).sections
  assert.equal(Math.round(decimalForm.ratio_pl * 1000), 120)
  assert.equal(Math.round(integerForm.ratio_pl * 1000), 12)
})

test('sections without milestones are told apart by their endpoints', () => {
  const { sections } = parseTmjaFiles([file([
    { ...A26_LOW, cumulD: '', cumulF: '' },
    { ...A26_HIGH, cumulD: '', cumulF: '' },
  ])])
  assert.equal(sections.length, 2)
})

