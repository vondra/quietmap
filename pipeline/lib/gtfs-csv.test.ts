/** GTFS row selection preserves CSV identities and propagates incomplete reads. */
import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { readCsvRows, readGtfsStopTimes } from './gtfs-csv.js'

const directory = mkdtempSync(join(tmpdir(), 'gtfs-csv-'))
after(() => rmSync(directory, { recursive: true, force: true }))

const quote = (value: string): string => `"${value.replaceAll('"', '""')}"`

test('active stop-time selection preserves quoted identities in any column and counts every data row', async () => {
  for (const tripColumn of [0, 2]) {
    const headers = ['stop_id', 'arrival_time', 'unused']
    headers.splice(tripColumn, 0, 'trip_id')
    const row = (tripId: string) => {
      const values = ['Praha, "hlavní"', '25:12:00', '']
      values.splice(tripColumn, 0, tripId)
      return values.map(quote).join(',')
    }
    writeFileSync(join(directory, 'stop_times.txt'),
      `\uFEFF${headers.map(quote).join(',')}\r\n\r\n${row('inactive')}\r\n${row('T,"1')}\r\n${row('inactive')}`)
    const selected: string[][] = []
    const count = await readGtfsStopTimes(directory, new Map([['T,"1', true]]), (columns, tripIdIndex) => {
      assert.deepEqual(columns, headers)
      assert.equal(tripIdIndex, tripColumn)
      return fields => { selected.push(fields) }
    })
    const expected = ['Praha, "hlavní"', '25:12:00', '']
    expected.splice(tripColumn, 0, 'T,"1')
    assert.equal(count, 3)
    assert.deepEqual(selected, [expected])
  }
})

test('file, header and row-consumer failures reject the read without later callbacks', async () => {
  await assert.rejects(readCsvRows(join(directory, 'missing.txt'), () => assert.fail('missing file yielded a row')), /ENOENT/)
  writeFileSync(join(directory, 'stop_times.txt'), 'trip_id,stop_id\nT,A\nT,B\n')
  const failure = new Error('consumer rejected source')
  for (const duringHeader of [true, false]) {
    let visited = 0
    await assert.rejects(readGtfsStopTimes(directory, new Map([['T', true]]), () => {
      if (duringHeader) throw failure
      return () => { visited++; throw failure }
    }), error => error === failure)
    assert.equal(visited, duringHeader ? 0 : 1)
  }
})
