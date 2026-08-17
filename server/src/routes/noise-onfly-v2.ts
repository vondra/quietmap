/**
 * On-the-fly noise computation v2 — unified Rust engine.
 */

import type { FastifyInstance } from 'fastify'
import { Worker } from 'node:worker_threads'
import {
  NoiseOnflyRequestError,
  NoiseOnflySupervisor,
} from '../engine/noise-onfly-supervisor.js'
import { prepareSourceReaderAddon } from '../engine/source-reader-addon.js'
import { EXPENSIVE_ROUTE_RATE_LIMIT } from '../rate-limit.js'
import { H3R4_DIR, SOURCE_READER_PATH } from '../runtime-paths.js'
import {
  PublishedLineModelUnavailableError,
  type PublishedLineModel,
} from '../published-line-model.js'

// Wire shape is built entirely in Rust (engine/source-reader/src/wire.rs).
// Node forwards the JSON string after one sentinel replace for
// `compute_time_ms`. Previously this route parsed, reshaped, and
// re-stringified — the cause of `provenance: null` and
// `total_lden_free: null` regressions (Rust struct renames silently
// diverged from Node's reshape). Single source of truth, ~30-50 ms saved.

const WORKER_URL = new URL('../workers/noise-onfly-worker.mjs', import.meta.url)
const NOISE_ONFLY_WORK_TIMEOUT_MS = Number(process.env.NOISE_ONFLY_WORK_TIMEOUT_MS || '30000')
const NOISE_ONFLY_QUEUE_TIMEOUT_MS = Number(process.env.NOISE_ONFLY_QUEUE_TIMEOUT_MS || '10000')
const NOISE_ONFLY_MAX_QUEUE = Number(process.env.NOISE_ONFLY_MAX_QUEUE || '8')
// Pool of NAPI workers — one popup computes on ONE worker thread; the pool
// size is the number of CONCURRENT visitors served without queueing. Each
// worker keeps its own loaded-area cache (mmap-backed, so RSS is mostly
// shared page cache). Default 8 (owner 2026-07-10, production sizing);
// override per box with NOISE_ONFLY_POOL_SIZE.
const NOISE_ONFLY_POOL_SIZE = Number(process.env.NOISE_ONFLY_POOL_SIZE || '8')

export type NoiseOnflyEngine = {
  checkReady: () => Promise<void>
  queryObstacleFootprints: (south: number, west: number, north: number, east: number) => Promise<string>
}

export async function noiseOnflyV2Routes(
  app: FastifyInstance,
  publishedLineModel: PublishedLineModel,
): Promise<NoiseOnflyEngine> {
  const supervisor = new NoiseOnflySupervisor({
    createWorker: () => {
      // Cheap when current, and self-heals if an operator removed the stable
      // copy while this process was alive.
      const sourceReaderNodePath = prepareSourceReaderAddon(SOURCE_READER_PATH)
      return new Worker(WORKER_URL, {
        workerData: {
          sourceReaderNodePath,
          h3r4Dir: H3R4_DIR,
        },
      })
    },
    maxQueue: NOISE_ONFLY_MAX_QUEUE,
    queueTimeoutMs: NOISE_ONFLY_QUEUE_TIMEOUT_MS,
    workTimeoutMs: NOISE_ONFLY_WORK_TIMEOUT_MS,
    poolSize: NOISE_ONFLY_POOL_SIZE,
    logger: (level, message, meta) => {
      if (meta) {
        app.log[level](meta, message)
        return
      }
      app.log[level](message)
    },
  })

  app.addHook('onClose', async () => {
    await supervisor.close()
  })

  app.get<{ Querystring: { lat?: string; lng?: string; full?: string } }>(
    '/api/noise-onfly-v2',
    // The popup compute is the most expensive public surface (one worker
    // thread per query) — rate-limited per client (owner directive 2026-07-15).
    { config: { rateLimit: EXPENSIVE_ROUTE_RATE_LIMIT } },
    async (request, reply) => {
      const lat = parseFloat(request.query.lat ?? '')
      const lng = parseFloat(request.query.lng ?? '')
      if (isNaN(lat) || isNaN(lng) || lat < -90 || lat > 90 || lng < -180 || lng > 180) {
        return reply.status(400).send({ error: 'valid lat and lng required' })
      }
      const full = request.query.full === '1' || request.query.full === 'true'

      const t0 = Date.now()
      const abortController = new AbortController()
      const onClose = () => {
        abortController.abort()
      }
      request.raw.once('close', onClose)

      try {
        // Fixed-size in-memory comparison only. Manifest reads and hashing are
        // owned by authenticated publish IPC, never by a visitor request.
        publishedLineModel.assertPopupAvailable()
        const resultJson = full
          ? await supervisor.queryNoiseAtPointUnfiltered(lat, lng, abortController.signal)
          : await supervisor.queryNoiseAtPoint(lat, lng, abortController.signal)
        const elapsed = Date.now() - t0

        // Rust builds the final wire shape directly (engine/source-reader/
        // src/wire.rs). Node injects only the wall-time measurement via a
        // single sentinel replace, then sends the string as-is — Fastify
        // forwards strings with explicit `application/json` content-type
        // without re-serializing (saves ~30-50 ms per popup vs the prior
        // parse + reshape + auto-stringify path).
        const sentinel = '"compute_time_ms":"__QM_COMPUTE_TIME_MS__"'
        const replaced = `"compute_time_ms":${elapsed}`
        const finalJson = resultJson.replace(sentinel, replaced)
        if (finalJson === resultJson) {
          throw new Error(`compute_time_ms sentinel missing from Rust output`)
        }
        if (finalJson.includes('__QM_COMPUTE_TIME_MS__')) {
          throw new Error(`compute_time_ms sentinel appeared more than once`)
        }
        return reply.type('application/json').send(finalJson)
      } catch (err) {
        if (abortController.signal.aborted || request.raw.aborted || request.raw.destroyed) {
          return
        }
        const error = err instanceof Error ? err : new Error(String(err))
        const message = error.message
        const statusCode = err instanceof PublishedLineModelUnavailableError
          ? 503
          : err instanceof NoiseOnflyRequestError
          ? err.statusCode
          : message.includes('timeout')
            ? 504
            : 500
        const publicMessage = err instanceof PublishedLineModelUnavailableError
          ? 'published line model unavailable'
          : message
        return reply.status(statusCode).send({ error: publicMessage })
      } finally {
        request.raw.removeListener('close', onClose)
      }
    }
  )

  return {
    checkReady: async () => supervisor.checkReady(),
    // The building-height debug overlay's data source (raster-tiles.ts) —
    // exact footprints + as-used heights straight from the popup worker's
    // obstacle store.
    queryObstacleFootprints: (south: number, west: number, north: number, east: number) =>
      supervisor.queryObstacleFootprints(south, west, north, east),
  }
}
