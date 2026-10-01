// GET /api/popup?lat=&lon=[&year=][&segments=1]: one click's answer streamed as it is refined
// (with the segments view's pieces when asked), one line of JSON
// per update (application/x-ndjson), each line flushed as `qm-popup` writes it. A failed
// computation ends the stream with one line `{"error": "..."}`; the updates before it are
// incomplete and must not be shown as the level.
import { PassThrough } from 'node:stream'
import type { FastifyInstance } from 'fastify'
import { EXPENSIVE_ROUTE_RATE_LIMIT } from '../rate-limit.ts'
import type { PopupRequest, PopupRunner } from '../popup-runner.ts'

/** The web-map (EPSG:3857) latitude limit, rounded inwards. */
export const MAX_LATITUDE = 85.05

const DECIMAL = /^[+-]?([0-9]+(\.[0-9]*)?|\.[0-9]+)([eE][+-]?[0-9]+)?$/

function coordinate(text: unknown): number | null {
  if (typeof text !== 'string' || !DECIMAL.test(text)) return null
  const value = Number(text)
  return Number.isFinite(value) ? value : null
}

/** The click's point and year, or what is wrong with the query. Longitude wraps to -180..180. */
export function parsePopupQuery(
  query: { lat?: unknown; lon?: unknown; year?: unknown; segments?: unknown },
  years: readonly string[],
): PopupRequest | string {
  const lat = coordinate(query.lat)
  if (lat === null || Math.abs(lat) > MAX_LATITUDE) return `lat must be a number within ±${MAX_LATITUDE}`
  const lon = coordinate(query.lon)
  if (lon === null) return 'lon must be a number'
  const year = query.year ?? years[0]
  if (typeof year !== 'string' || !years.includes(year)) return `year must be one of ${years.join(', ')}`
  if (query.segments !== undefined && query.segments !== '1') return 'segments must be 1'
  return {
    year,
    lat,
    lon: ((((lon + 180) % 360) + 360) % 360) - 180,
    ...(query.segments === '1' ? { segments: true } : {}),
  }
}

export async function popupRoutes(
  app: FastifyInstance,
  { runner, years }: { runner: PopupRunner; years: readonly string[] },
): Promise<void> {
  app.get<{ Querystring: { lat?: string; lon?: string; year?: string; segments?: string } }>('/api/popup', {
    // Never compressed: a compressor holds lines back until its buffer fills.
    compress: false,
    // A HEAD request would compute a click nobody reads.
    exposeHeadRoute: false,
    config: { rateLimit: EXPENSIVE_ROUTE_RATE_LIMIT },
  }, async (request, reply) => {
    const parsed = parsePopupQuery(request.query, years)
    if (typeof parsed === 'string') return reply.code(400).send({ error: parsed })

    const stream = new PassThrough()
    const cancel = runner.submit(parsed, {
      onLine: (line) => {
        if (!stream.destroyed) stream.write(`${line}\n`)
      },
      onEnd: (failure) => {
        // A destroyed stream is a visitor who left: the child was killed for it, nothing failed.
        if (stream.destroyed) return
        if (failure) {
          request.log.warn({ click: parsed, detail: failure.detail }, 'popup failed')
          stream.write(`${JSON.stringify({ error: failure.message })}\n`)
        }
        stream.end()
      },
    })
    if (!cancel) {
      return reply.code(503).header('Retry-After', '1')
        .send({ error: 'The noise service is busy. Try again in a moment.' })
    }
    // The stream closes when it ended or when the client disconnected (Fastify destroys it);
    // cancelling a finished run does nothing.
    stream.on('close', cancel)
    return reply
      .type('application/x-ndjson; charset=utf-8')
      // No cache and no proxy may keep or re-encode the stream; nginx-style proxies must not buffer it.
      .header('Cache-Control', 'no-store, no-transform')
      .header('X-Accel-Buffering', 'no')
      .send(stream)
  })
}
