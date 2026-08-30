import Fastify from 'fastify'
import type { FastifyError, FastifyInstance, FastifyPluginAsync } from 'fastify'
import compress from '@fastify/compress'
import rateLimit from '@fastify/rate-limit'
import { randomUUID } from 'node:crypto'
import { isLoopbackClient, rateLimitClientKey } from './rate-limit.js'
import { searchRoutes } from './routes/search.js'
import { noiseOnflyV2Routes } from './routes/noise-onfly-v2.js'
import { isochronRoutes } from './routes/isochron.js'
import { docsRoutes } from './routes/docs.js'
import { propertiesRoutes } from './routes/properties.js'
import { stayRoutes } from './routes/stay.js'
import { rasterTileRoutes } from './routes/raster-tiles.js'
import { aircraftRoutes } from './routes/aircraft.js'
import { heatmapPmtilesRoutes } from './routes/heatmap-pmtiles.js'
import { tilesManifestRoutes } from './routes/tiles-manifest.js'
import { initialViewRoutes } from './routes/initial-view.js'
import { validationViewRoutes } from './routes/validation-view.js'
import { healthRoutes } from './routes/health.js'
import { createReadinessCheck, type ReadinessCheck } from './runtime-readiness.js'
import { requireLocalPeer } from './internal-access.js'
import { importOptionalOpsModule } from './ops-routes.js'

// Deliberately identifies only this Node process, not its build or data. Long
// validation runs use it to reject results spanning a restart/deploy.
const INSTANCE_ID = randomUUID()

export type BuildAppOptions = {
  logger?: boolean
  readinessCheck?: ReadinessCheck
  enableClusterRoutes?: boolean
  /** Simulates ops-module absence in tests; the production gate is file presence. */
  importOpsModule?: typeof importOptionalOpsModule
  noIndex?: boolean
  preloadRuntimeData?: boolean
}

/** Dev checkouts expose the loopback-gated dashboard by default; an explicit flag still wins. */
export function clusterRoutesEnabled(env: NodeJS.ProcessEnv = process.env): boolean {
  if (env.ENABLE_CLUSTER_ROUTES != null) return env.ENABLE_CLUSTER_ROUTES === '1'
  return /^dev[123]$/.test(env.TILE_ENV ?? '')
}

export async function buildApp(opts: BuildAppOptions = {}): Promise<FastifyInstance> {
  const app = Fastify({
    logger: opts.logger ?? false,
    // The access log lives in Caddy (full URIs, 30-day rotation, aggregated
    // into anonymous product stats); per-request pino lines only duplicated
    // it. Keep this logger for startup and crash diagnostics instead.
    disableRequestLogging: true,
    // Never trust arbitrary forwarding headers. Caddy reaches Fastify from
    // loopback; only that hop may supply the public client's address.
    trustProxy: ['127.0.0.1', '::1'],
  })

  // disableRequestLogging also silences the default error handler, which would
  // make 5xx stack traces invisible (Caddy sees only the status). Log server
  // errors here; client errors (4xx) stay out — Caddy already records them.
  // Like the default handler, the status is set BEFORE send: @fastify/rate-limit
  // throws its 429 payload as a plain object, and reply.send() of a non-Error
  // keeps the current status (200) unless it is set here first.
  app.setErrorHandler((error: FastifyError, request, reply) => {
    const statusCode = error.statusCode ?? 500
    if (statusCode >= 500) {
      request.log.error({ err: error }, error.message)
    }
    reply.status(statusCode).send(error)
  })

  app.addHook('onSend', async (_request, reply, payload) => {
    reply.header('X-0db-Instance', INSTANCE_ID)
    return payload
  })

  if (opts.noIndex ?? process.env.ROBOTS_NOINDEX === '1') {
    app.addHook('onSend', async (_request, reply, payload) => {
      reply.header('X-Robots-Tag', 'noindex, nofollow, noarchive')
      return payload
    })
  }

  await app.register(compress)

  // Opt-in only (global: false): solely the expensive endpoints — popup noise
  // compute + Photon geocode proxies — carry `config.rateLimit` (see
  // rate-limit.ts). Tile and asset routes stay unlimited by construction.
  await app.register(rateLimit, {
    global: false,
    // Local unproxied callers (check-popup/check-world parity skills, smoke
    // tests) are exempt — see isLoopbackClient for why this is not spoofable.
    allowList: (request) => isLoopbackClient(request.ip),
    keyGenerator: (request) => rateLimitClientKey(request.ip),
    errorResponseBuilder: (_request, context) => ({
      statusCode: 429,
      error: 'Too Many Requests',
      message: `Rate limit exceeded (${context.max} requests per second). Retry shortly.`,
    }),
  })

  await app.register(searchRoutes)
  // Registered directly because the readiness probe is a capability returned
  // by the live supervisor instance; Fastify plugin registration discards a
  // plugin's return value.
  const engine = await noiseOnflyV2Routes(app)
  const engineProbe = engine.checkReady
  await app.register(isochronRoutes)
  await app.register(docsRoutes)
  await app.register(propertiesRoutes)
  await app.register(stayRoutes)
  await app.register(rasterTileRoutes, {
    preloadRuntimeData: opts.preloadRuntimeData ?? false,
    queryObstacleFootprints: engine.queryObstacleFootprints,
  })
  await app.register(aircraftRoutes)
  // Heatmap tiles: immutable pmtiles builds addressed by build-id, discovered
  // via the manifest (storage redesign 2026-07). The loose-file route is gone
  // with the loose trees.
  await app.register(heatmapPmtilesRoutes)
  await app.register(tilesManifestRoutes)
  await app.register(initialViewRoutes)
  await app.register(validationViewRoutes)
  // Inbound-mail archive webhook (Cloudflare Email Worker → the ops mail host), ops-only:
  // the public distribution ships without the file — its presence is the gate.
  const importOpsModule = opts.importOpsModule ?? importOptionalOpsModule
  const mailInboundModule = await importOpsModule<{ mailInboundRoutes: FastifyPluginAsync }>(
    './routes/mail-inbound.js',
  )
  if (mailInboundModule) {
    // Encapsulated so its raw MIME content-type parser never touches the JSON routes.
    await app.register(mailInboundModule.mailInboundRoutes)
  } else {
    app.log.info('ops route module ./routes/mail-inbound.js absent; skipping registration')
  }

  const readiness = opts.readinessCheck ?? createReadinessCheck({ engineProbe })
  await healthRoutes(app, readiness)

  const enableClusterRoutes = opts.enableClusterRoutes
    ?? clusterRoutesEnabled()
  if (enableClusterRoutes) {
    // Dynamic import means a public-only process never even loads code that
    // reads SSH inventory, worker logs, costs, or cluster telemetry.
    const clusterModule = await importOpsModule<{ clusterRoutes: FastifyPluginAsync }>('./routes/cluster.js')
    if (!clusterModule) {
      app.log.info('ops route module ./routes/cluster.js absent; cluster dashboard not registered')
    } else {
      // Internal admin area under the /a/ prefix — cluster now (→ /a/cluster +
      // /a/api/cluster/status), other admins later. TWO layers of access control, because
      // it surfaces box IPs, telemetry, and $ costs: an authenticating reverse proxy in
      // front (the password), AND requireLocalPeer here so a direct, proxy-bypassing
      // connection can't reach it either. The proxy connects from loopback, so authed
      // requests + the local TUI pass; the public map stays open.
      // NOTE the prefix must match cluster-page.ts's poll URL and scripts/cluster-dash.py.
      await app.register(async (adminApp) => {
        adminApp.addHook('onRequest', requireLocalPeer)
        await adminApp.register(clusterModule.clusterRoutes)
        // Anonymous web analytics (/a/stats + /a/api/stats/*) — same /a scope;
        // reads only the aggregate SQLite DB and bounded tails of the access log.
        const webStatsModule = await importOpsModule<{ webStatsRoutes: FastifyPluginAsync }>(
          './routes/web-stats.js',
        )
        if (webStatsModule) {
          await adminApp.register(webStatsModule.webStatsRoutes)
        } else {
          app.log.info('ops route module ./routes/web-stats.js absent; skipping registration')
        }
      }, { prefix: '/a' })
    }
  }

  return app
}
