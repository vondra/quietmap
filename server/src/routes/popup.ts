// GET /api/popup?lat=&lon=[&year=][&source=ID,...[&piece=K]]: one click's answer streamed as it is
// refined (with an opened row's sound path, or one of its pieces ray by ray, when asked), one line
// of JSON
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

/** A query's decimal number, or null for anything else (`Number` alone reads '' as 0). */
export function coordinate(text: unknown): number | null {
  if (typeof text !== 'string' || !DECIMAL.test(text)) return null
  const value = Number(text)
  return Number.isFinite(value) ? value : null
}

/** An opened row's parts: one to eight group ids of 16 hex digits. */
const SOURCE = /^[0-9a-f]{16}(,[0-9a-f]{16}){0,7}$/
/** A listed piece's rank: the popup lists 24. */
const PIECE = /^([0-9]|1[0-9]|2[0-3])$/

/** The click's point and year, or what is wrong with the query. Longitude wraps to -180..180. */
export function parsePopupQuery(
  query: { lat?: unknown; lon?: unknown; year?: unknown; source?: unknown; piece?: unknown },
  years: readonly string[],
): PopupRequest | string {
  const lat = coordinate(query.lat)
  if (lat === null || Math.abs(lat) > MAX_LATITUDE) return `lat must be a number within ±${MAX_LATITUDE}`
  const lon = coordinate(query.lon)
  if (lon === null) return 'lon must be a number'
  const year = query.year ?? years[0]
  if (typeof year !== 'string' || !years.includes(year)) return `year must be one of ${years.join(', ')}`
  if (query.source !== undefined && (typeof query.source !== 'string' || !SOURCE.test(query.source))) {
    return 'source must be one to eight ids of 16 hex digits'
  }
  if (query.piece !== undefined && (query.source === undefined || typeof query.piece !== 'string' || !PIECE.test(query.piece))) {
    return 'piece must be 0 to 23, with a source'
  }
  return {
    year,
    lat,
    // Only a longitude outside it wraps: the arithmetic moves an in-range one by 1e-14, enough to
    // change the click's sampling seed against the same point asked of the popup directly.
    lon: lon >= -180 && lon < 180 ? lon : ((((lon + 180) % 360) + 360) % 360) - 180,
    ...(typeof query.source === 'string' ? { source: query.source.split(',') } : {}),
    ...(typeof query.piece === 'string' ? { piece: Number(query.piece) } : {}),
  }
}

export async function popupRoutes(
  app: FastifyInstance,
  { runner, years }: { runner: PopupRunner; years: readonly string[] },
): Promise<void> {
  app.get<{ Querystring: { lat?: string; lon?: string; year?: string; source?: string; piece?: string } }>('/api/popup', {
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
