/** Route GTFS countries concurrently, then publish whole snapshots in manifest order. */

import { fork } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import { resolve } from 'node:path'
import { getHeapStatistics } from 'node:v8'
import type { GlobalGtfsCountryOptions } from '../enrich-railway-europe.js'
import { countryGtfsBbox, feedsForRegistry } from './railway-gtfs-feeds.js'
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

/** The chain subprocess retains its existing enrichment/admin locks until all children exit. */
export async function runGtfsCountries(options: GlobalGtfsCountryOptions): Promise<void> {
  const countries = options.country.toUpperCase().split(',')
  if (countries.some(country => !/^[A-Z]{2}$/.test(country)) || new Set(countries).size !== countries.length) {
    throw new Error('GTFS countries must be distinct ISO2 codes')
  }
  // Reserve a second heap-equivalent for Arrow/native memory; this is admission,
  // not a claim that V8 limits the complete RSS. Four bounds retained country results.
  const prepared = resolve(options.preparedDirectory)
  const squares = new Set(countries.flatMap(country =>
    listPreparedSquares(prepared, countryGtfsBbox(country, feedsForRegistry(options.registry ?? 'global')), 'railways.arrow')))
  {
    using topology = new SourceTransportTopology(prepared)
    for (const square of squares) {
      restoreRailwayParentsForEnrichment(resolve(prepared, square, 'railways.arrow'), prepared, square, topology)
    }
  }
  for (let offset = 0; offset < countries.length;) {
    // The builder releases sibling producers' memory between country batches.
    const workers = Math.min(4, countries.length - offset,
      workerCount(process.env, 2 * getHeapStatistics().heap_size_limit))
    const batch = countries.slice(offset, offset + workers)
    console.log(JSON.stringify({ phase: 'rail-country-routing', countries: batch, workers, squares: squares.size }))
    let rejectFailure!: (error: Error) => void
    const failed = new Promise<never>((_resolve, reject) => { rejectFailure = reject })
    const children = batch.map(country => {
      const child = fork(fileURLToPath(new URL('../enrich-railway-europe.ts', import.meta.url)), [
        '--source-dir', options.sourceDirectory, '--prepared-dir', prepared,
        '--cache-dir', options.cacheDirectory, '--country', country,
        '--registry', options.registry ?? 'global', '--as-of-date', options.asOfDate,
      ], { stdio: ['ignore', 'inherit', 'inherit', 'ipc'] })
      let published = false
      const ready = new Promise<void>(accept => {
        child.once('message', message => {
          if (message === 'ready') accept()
          else rejectFailure(new Error(`${country}: invalid railway worker response`))
        })
      })
      child.once('error', rejectFailure)
      const completed = new Promise<void>(accept => child.once('close', (code, signal) => {
        if (code !== 0 || !published) rejectFailure(new Error(`${country}: railway worker exited ${code ?? signal}`))
        accept()
      }))
      return { child, ready, completed, publish: () => {
        published = true
        child.send('publish', error => { if (error) rejectFailure(error) })
      } }
    })
    try {
      // A failed input in this batch cannot publish any of its siblings.
      await Promise.race([Promise.all(children.map(worker => worker.ready)), failed])
      for (const worker of children) {
        worker.publish()
        await Promise.race([worker.completed, failed])
      }
    } finally {
      for (const worker of children) if (worker.child.exitCode === null) worker.child.kill()
      await Promise.all(children.map(worker => worker.completed))
    }
    offset += workers
  }
}
