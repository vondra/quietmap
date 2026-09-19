/** Route GTFS countries concurrently, publish ready snapshots through one writer. */

import { fork } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import { statSync } from 'node:fs'
import { resolve } from 'node:path'
import { getHeapStatistics } from 'node:v8'
import type { GlobalGtfsCountryOptions } from '../enrich-railway-europe.js'
import { countryGtfsBbox, feedsForRegistry, gtfsSourceDirectories } from './railway-gtfs-feeds.js'
import { listPreparedSquares } from './prepared-grid.js'
import { restoreRailwayParentsForEnrichment } from './rail-parent.js'
import { SourceTransportTopology } from './transport-topology.js'
import { workerCount } from './square-pool.js'

export function awaitCountryPublication(): Promise<void> {
  return new Promise((accept, reject) => {
    process.once('message', message => {
      if (message !== 'publish') reject(new Error('invalid railway publication request'))
      else accept()
    })
    process.send!('ready', error => { if (error) reject(error) })
  })
}

/**
 * Routing time follows the timetable size: DE's 4.1 GB `stop_times.txt` took 1,562 s and, started
 * late in alphabetical order, ended the world batch alone. The largest timetable starts first.
 */
export function countriesLargestTimetableFirst(
  countries: readonly string[], options: Pick<GlobalGtfsCountryOptions, 'sourceDirectory' | 'cacheDirectory' | 'registry'>,
): string[] {
  const feeds = feedsForRegistry(options.registry ?? 'global')
  const stopTimesBytes = new Map(countries.map(country => [country, feeds
    .filter(feed => feed.country === country)
    .flatMap(feed => gtfsSourceDirectories(resolve(options.sourceDirectory), feed, resolve(options.cacheDirectory)))
    .reduce((bytes, directory) => bytes + statSync(resolve(directory, 'stop_times.txt')).size, 0)]))
  return [...countries].sort((left, right) => stopTimesBytes.get(right)! - stopTimesBytes.get(left)!)
}

/** The chain subprocess retains its existing enrichment/admin locks until all children exit. */
export async function runGtfsCountries(options: GlobalGtfsCountryOptions): Promise<void> {
  const requested = options.country.toUpperCase().split(',')
  if (requested.some(country => !/^[A-Z]{2}$/.test(country)) || new Set(requested).size !== requested.length) {
    throw new Error('GTFS countries must be distinct ISO2 codes')
  }
  const countries = countriesLargestTimetableFirst(requested, options)
  // Reserve a second heap-equivalent for Arrow/native memory; this is admission,
  // not a claim that V8 limits the complete RSS. Four bounds retained country results.
  const prepared = resolve(options.preparedDirectory)
  const squares = new Set(countries.flatMap(country =>
    listPreparedSquares(prepared, countryGtfsBbox(country, feedsForRegistry(options.registry ?? 'global')), 'railways.arrow')))
  {
    using topology = new SourceTransportTopology(prepared)
    for (const square of squares) {
      restoreRailwayParentsForEnrichment(resolve(prepared, square, 'railways.arrow'), square, topology)
    }
  }
  let failure: Error | undefined
  let rejectFailure!: (error: Error) => void
  const failed = new Promise<never>((_resolve, reject) => { rejectFailure = reject })
  const fail = (error: Error) => { failure ??= error; rejectFailure(failure) }
  const startCountry = (country: string) => {
    const child = fork(fileURLToPath(new URL('../enrich-railway-europe.ts', import.meta.url)), [
      '--source-dir', options.sourceDirectory, '--prepared-dir', prepared,
      '--cache-dir', options.cacheDirectory, '--country', country,
      '--registry', options.registry ?? 'global', '--as-of-date', options.asOfDate,
    ], { stdio: ['ignore', 'inherit', 'inherit', 'ipc'] })
    let published = false
    const ready = new Promise<string>(accept => {
      child.once('message', message => {
        if (message === 'ready') accept(country)
        else fail(new Error(`${country}: invalid railway worker response`))
      })
    })
    child.once('error', fail)
    const completed = new Promise<void>(accept => child.once('close', (code, signal) => {
      if (code !== 0 || !published) fail(new Error(`${country}: railway worker exited ${code ?? signal}`))
      accept()
    }))
    return { country, child, ready, completed, publish: () => {
      published = true
      child.send('publish', error => { if (error) fail(error) })
    } }
  }
  const active: Array<ReturnType<typeof startCountry>> = []
  let next = 0
  try {
    while (next < countries.length || active.length > 0) {
      // Reconsider released memory whenever a completed country frees a slot.
      const workers = Math.min(4, workerCount(process.env, 2 * getHeapStatistics().heap_size_limit))
      while (!failure && next < countries.length && active.length < workers) {
        const country = countries[next++]
        active.push(startCountry(country))
        console.log(JSON.stringify({ phase: 'rail-country-routing', country, workers, squares: squares.size }))
      }
      if (failure) throw failure
      const country = await Promise.race([...active.map(worker => worker.ready), failed])
      if (failure) throw failure
      const worker = active.find(worker => worker.country === country)!
      worker.publish()
      await Promise.race([worker.completed, failed])
      if (failure) throw failure
      active.splice(active.indexOf(worker), 1)
    }
  } finally {
    // Completed countries persist on later failure, as in serial country replay.
    for (const worker of active) if (worker.child.exitCode === null) worker.child.kill()
    await Promise.all(active.map(worker => worker.completed))
  }
}
