// Entry point: read the configuration, build the app, serve the built frontend, listen.
import { resolve } from 'node:path'
import { buildApp } from './app.ts'
import { readConfig } from './config.ts'
import { registerWeb } from './web.ts'

/** The built frontend of this checkout (`npm --prefix frontend run build`). */
const FRONTEND_DIST = resolve(import.meta.dirname, '..', '..', 'frontend', 'dist')

const config = readConfig()
const app = await buildApp(config, { logger: true })

// A request-scoped network error must never take down the whole map: a
// double-send (ERR_HTTP_HEADERS_SENT) or a client that vanished mid-response
// (EPIPE/ECONNRESET) would otherwise escape as an uncaughtException and exit
// the process. Swallow ONLY these known request-scoped codes (log + continue);
// exit for anything else so a genuine bug still fails fast and the service
// manager restarts the server.
const BENIGN_REQUEST_ERROR_CODES = new Set([
  'ERR_HTTP_HEADERS_SENT', 'ERR_STREAM_WRITE_AFTER_END', 'ERR_STREAM_DESTROYED',
  'ERR_STREAM_ALREADY_FINISHED', 'EPIPE', 'ECONNRESET',
])
const benignRequestError = (value: unknown): boolean =>
  value != null && typeof value === 'object' && 'code' in value
  && BENIGN_REQUEST_ERROR_CODES.has(String((value as { code?: unknown }).code))
process.on('uncaughtException', (error) => {
  if (benignRequestError(error)) {
    app.log.warn({ err: error }, 'swallowed benign request-scoped error — server stays up')
    return
  }
  app.log.fatal(error, 'uncaughtException — exiting for restart')
  process.exit(1)
})
process.on('unhandledRejection', (reason) => {
  if (benignRequestError(reason)) {
    app.log.warn({ err: reason }, 'swallowed benign request-scoped rejection — server stays up')
    return
  }
  app.log.fatal({ reason }, 'unhandledRejection — exiting for restart')
  process.exit(1)
})

await registerWeb(app, FRONTEND_DIST)

for (const signal of ['SIGTERM', 'SIGINT'] as const) {
  process.once(signal, async () => {
    app.log.info({ signal }, 'server shutting down')
    try {
      await app.close()
      process.exit(0)
    } catch (error) {
      app.log.error(error)
      process.exit(1)
    }
  })
}

try {
  await app.listen({ port: config.port, host: config.host })
} catch (error) {
  app.log.error(error)
  process.exit(1)
}
