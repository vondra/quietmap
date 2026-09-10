/** Optional outdoor layer preview; never occupies the exact popup worker pool. */
import type { FastifyInstance } from 'fastify'
import { Worker } from 'node:worker_threads'
import { NoiseOnflySupervisor } from '../engine/noise-onfly-supervisor.js'
import { pinSourceReaderAddonOnMainThread, prepareSourceReaderAddon } from '../engine/source-reader-addon.js'
import { EXPENSIVE_ROUTE_RATE_LIMIT } from '../rate-limit.js'
import { PREPARED_YEAR_DIR, SOURCE_READER_PATH, SURFACE_CORNERS_DIR } from '../runtime-paths.js'

type QueryPreview = (lat: number, lng: number, signal?: AbortSignal) => Promise<string>

export async function noisePreviewRoutes(app: FastifyInstance, queryPreview?: QueryPreview): Promise<void> {
  if (!queryPreview && SURFACE_CORNERS_DIR) {
    const supervisor = new NoiseOnflySupervisor({
      createWorker: () => {
        // The preview pool spawns before the popup pool: pin the addon here
        // too, or its first worker dlopens it unpinned (the dlclose SIGSEGV class).
        const sourceReaderNodePath = prepareSourceReaderAddon(SOURCE_READER_PATH)
        pinSourceReaderAddonOnMainThread(sourceReaderNodePath)
        const worker = new Worker(new URL('../workers/noise-onfly-worker.mjs', import.meta.url), {
          workerData: {
            sourceReaderNodePath,
            preparedYearDir: PREPARED_YEAR_DIR,
            surfaceCornersRoot: SURFACE_CORNERS_DIR,
          },
        })
        const ready = new Promise<void>((resolve, reject) => {
          const cleanup = () => {
            worker.removeListener('message', onMessage)
            worker.removeListener('error', onError)
            worker.removeListener('exit', onExit)
          }
          const onMessage = (message: { initialized?: boolean; available?: boolean }) => {
            if (!message.initialized) return
            cleanup()
            if (!message.available) app.log.warn('surface corners unavailable for the pinned prepared generation')
            resolve()
          }
          const onError = (error: Error) => { cleanup(); reject(error) }
          const onExit = (code: number) => onError(new Error(`preview worker exited during initialization: ${code}`))
          worker.on('message', onMessage)
          worker.once('error', onError)
          worker.once('exit', onExit)
        })
        return Object.assign(worker, { ready })
      },
      poolSize: 1,
      maxQueue: 8,
      queueTimeoutMs: 250,
      workTimeoutMs: 1500,
    })
    supervisor.warmWorkers()
    queryPreview = (lat, lng, signal) => supervisor.querySurfaceCornerPreview(lat, lng, signal)
    app.addHook('onClose', async () => supervisor.close())
  }
  const query = queryPreview ?? (async () => 'null')
  app.get<{ Querystring: { lat?: string; lng?: string } }>(
    '/api/noise-preview',
    { config: { rateLimit: EXPENSIVE_ROUTE_RATE_LIMIT } },
    async (request, reply) => {
      const lat = request.query.lat?.trim() ? Number(request.query.lat) : NaN
      const lng = request.query.lng?.trim() ? Number(request.query.lng) : NaN
      if (!Number.isFinite(lat) || !Number.isFinite(lng) || lat < -90 || lat > 90 || lng < -180 || lng > 180) {
        return reply.status(400).send({ error: 'valid lat and lng required' })
      }
      const controller = new AbortController()
      const onClose = () => controller.abort()
      request.raw.once('close', onClose)
      try {
        return reply.type('application/json').send(await query(lat, lng, controller.signal))
      } catch (error) {
        if (controller.signal.aborted) return
        // A stale, unavailable or busy preview leaves the independent exact request intact.
        app.log.warn({ error: error instanceof Error ? error.message : String(error) }, 'surface preview unavailable')
        return reply.type('application/json').send('null')
      } finally {
        request.raw.removeListener('close', onClose)
      }
    },
  )
}
