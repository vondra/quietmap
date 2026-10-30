// The HTTP API: the streamed popup, the geocoder proxies, the heatmap tiles, the data layers, the
// places to stay and the optional sign-in with a signed-in visitor's recordings and photos, behind
// per-client rate limits on the expensive routes. The static frontend is added by server.ts (web.ts).
import Fastify from 'fastify'
import type { FastifyError, FastifyInstance } from 'fastify'
import compress from '@fastify/compress'
import rateLimit from '@fastify/rate-limit'
import type { Config } from './config.ts'
import { isLoopbackClient, rateLimitClientKey } from './rate-limit.ts'
import { PopupRunner } from './popup-runner.ts'
import { popupRoutes } from './routes/popup.ts'
import { searchRoutes } from './routes/search.ts'
import { isochronRoutes } from './routes/isochron.ts'
import { heatmapPmtilesRoutes } from './routes/heatmap-pmtiles.ts'
import { heatmapManifestRoutes } from './routes/heatmap-manifest.ts'
import { rasterRoutes } from './routes/raster.ts'
import { sourceOutlineRoutes } from './routes/source-outline.ts'
import { stayRoutes } from './routes/stay.ts'
import { accountRoutes } from './routes/account.ts'
import { mediaRoutes } from './routes/media.ts'
import { AccountStore } from './accounts/store.ts'
import { resendSender } from './accounts/mail.ts'

/** Clicks waiting per computing slot: beyond that a visitor waits longer than a retry takes. */
const POPUP_QUEUE_PER_SLOT = 2
/** A click still computing after this is killed and answered with an error. */
const POPUP_TIMEOUT_MS = 30_000

export type AppConfig = Pick<Config, 'popupBin' | 'rasterBin' | 'preparedDir' | 'years' | 'popupConcurrency' | 'tilesDir' | 'noIndex' | 'photonUrl' | 'valhallaUrl' | 'stay22' | 'accounts'>

export async function buildApp(
  config: AppConfig,
  { logger = false, popupTimeoutMs = POPUP_TIMEOUT_MS }: { logger?: boolean; popupTimeoutMs?: number } = {},
): Promise<FastifyInstance> {
  const app = Fastify({
    logger,
    // The access log belongs to the reverse proxy; per-request lines here would
    // only duplicate it. This logger is for startup, failures and crashes.
    disableRequestLogging: true,
    // Never trust arbitrary forwarding headers: only a proxy on loopback may
    // supply the public client's address.
    trustProxy: ['127.0.0.1', '::1'],
  })

  // disableRequestLogging also silences the default error handler, which would
  // make 5xx stack traces invisible. Log server errors here; client errors (4xx)
  // stay out. Like the default handler, the status is set BEFORE send:
  // @fastify/rate-limit throws its 429 payload as a plain object, and
  // reply.send() of a non-Error keeps the current status (200) unless it is set here first.
  app.setErrorHandler((error: FastifyError, request, reply) => {
    const statusCode = error.statusCode ?? 500
    if (statusCode >= 500) {
      request.log.error({ err: error }, error.message)
    }
    reply.status(statusCode).send(error)
  })

  if (config.noIndex) {
    app.addHook('onSend', async (_request, reply, payload) => {
      reply.header('X-Robots-Tag', 'noindex, nofollow, noarchive')
      return payload
    })
  }

  await app.register(compress)

  // Opt-in only (global: false): solely the expensive routes carry `config.rateLimit`
  // (see rate-limit.ts). Tile and asset routes stay unlimited by construction.
  await app.register(rateLimit, {
    global: false,
    allowList: (request) => isLoopbackClient(request.ip),
    keyGenerator: (request) => rateLimitClientKey(request.ip),
    errorResponseBuilder: (_request, context) => ({
      statusCode: 429,
      error: 'Too Many Requests',
      message: `Rate limit exceeded, retry in ${context.after}`,
    }),
  })

  const runner = new PopupRunner({
    popupBin: config.popupBin,
    preparedDir: config.preparedDir,
    concurrency: config.popupConcurrency,
    queueLength: config.popupConcurrency * POPUP_QUEUE_PER_SLOT,
    timeoutMs: popupTimeoutMs,
  })
  await app.register(popupRoutes, { runner, years: config.years })
  await app.register(sourceOutlineRoutes, { preparedDir: config.preparedDir, years: config.years })
  await app.register(searchRoutes, { photonUrl: config.photonUrl })
  await app.register(isochronRoutes, { valhallaUrl: config.valhallaUrl })
  await app.register(heatmapPmtilesRoutes, { tilesDir: config.tilesDir })
  await app.register(heatmapManifestRoutes, { tilesDir: config.tilesDir })
  await app.register(rasterRoutes, { rasterBin: config.rasterBin, preparedDir: config.preparedDir, year: config.years[0] })
  await app.register(stayRoutes, { stay22: config.stay22 })
  const accounts = config.accounts && {
    store: new AccountStore(config.accounts.database),
    sendLink: resendSender(config.accounts.mailKeyFile, config.accounts.mailFrom),
    origin: config.accounts.origin,
  }
  if (accounts) app.addHook('onClose', async () => accounts.store.close())
  await app.register(accountRoutes, { accounts })
  await app.register(mediaRoutes, { accounts })
  return app
}
