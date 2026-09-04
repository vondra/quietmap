import { buildApp } from './app.js'
import { registerWeb } from './web.js'
import { FRONTEND_DIST } from './runtime-paths.js'

const app = await buildApp({ logger: true })

// Crash-proofing invariant: a request-scoped network error must NEVER take down
// the whole map. A single bad reply (a double-send → ERR_HTTP_HEADERS_SENT, or a
// client that vanished mid-response → EPIPE/ECONNRESET) used to escape as an
// uncaughtException and exit the process — killing the live map (systemd then
// restarted it ~3 s later, a public 502 blip on every such hit, 2026-07-13).
// Swallow ONLY these known request-scoped codes (log + continue); re-exit for
// anything else so a genuine bug still fails fast → systemd Restart=on-failure.
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

const portText = process.env.PORT ?? '8501'
if (!/^[0-9]{1,5}$/.test(portText) || Number(portText) < 1 || Number(portText) > 65535) {
  throw new Error(`invalid PORT ${JSON.stringify(portText)} (expected 1-65535)`)
}
const port = Number(portText)
const host = process.env.HOST || '0.0.0.0'

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
  await app.listen({ port, host })
} catch (error) {
  app.log.error(error)
  process.exit(1)
}
