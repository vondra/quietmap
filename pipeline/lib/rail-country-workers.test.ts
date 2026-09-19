/** Country workers start in descending timetable size, summed over every feed of a country. */

import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { test } from 'node:test'
import { countriesLargestTimetableFirst } from './rail-country-workers.js'

function writeFeed(sourceDirectory: string, feedId: string, stopTimesBytes: number): void {
  const directory = join(sourceDirectory, feedId)
  mkdirSync(directory, { recursive: true })
  for (const name of ['stops.txt', 'trips.txt', 'routes.txt']) writeFileSync(join(directory, name), '')
  writeFileSync(join(directory, 'stop_times.txt'), 'x'.repeat(stopTimesBytes))
}

test('the largest stop_times total starts first; equal and missing timetables keep the requested order', () => {
  const sourceDirectory = mkdtempSync(join(tmpdir(), 'rail-country-order-'))
  try {
    writeFeed(sourceDirectory, 'be', 300)
    writeFeed(sourceDirectory, 'fi', 100)
    writeFeed(sourceDirectory, 'fr', 250)
    writeFeed(sourceDirectory, 'fr-idf', 150)
    writeFeed(sourceDirectory, 'lu', 100)
    const options = { sourceDirectory, cacheDirectory: join(sourceDirectory, 'cache'), registry: 'global' as const }
    assert.deepEqual(
      countriesLargestTimetableFirst(['LU', 'CA', 'FI', 'FR', 'BE'], options),
      ['FR', 'BE', 'LU', 'FI', 'CA'],
    )
  } finally {
    rmSync(sourceDirectory, { recursive: true, force: true })
  }
})
