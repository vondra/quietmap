// GET /api/isochron: the area reachable from a point in some minutes, on foot and by car, from the
// Valhalla router the deployment names (QM_VALHALLA_URL), as a FeatureCollection with a polygon a
// mode, the car's first so that the walk's draws over it; a mode the router failed is left out, the
// others kept. Only the point and the minutes leave.
import type { FastifyInstance } from 'fastify'
import { EXPENSIVE_ROUTE_RATE_LIMIT } from '../rate-limit.ts'
import { coordinate, wrappedLongitude } from './popup.ts'

/** Valhalla's costing of each mode, in the order the polygons are drawn. */
const COSTING = { car: 'auto', walk: 'pedestrian' } as const
type Mode = keyof typeof COSTING
const MINUTES_MAX = 120
const ROUTER_TIMEOUT_MS = 15_000
/** Who asks: the public router asks a published application to name itself (its X-Client-Id). */
const CLIENT = { 'X-Client-Id': 'quietmap.org', 'User-Agent': 'QuietMap (https://quietmap.org)' }

export async function isochronRoutes(app: FastifyInstance, { valhallaUrl }: { valhallaUrl: string }) {
  app.get<{ Querystring: Record<string, unknown> }>('/api/isochron', {
    config: { rateLimit: EXPENSIVE_ROUTE_RATE_LIMIT },
  }, async (request, reply) => {
    const [lat, lng, minutes] = [request.query.lat, request.query.lng, request.query.minutes].map(coordinate)
    const asked = new Set(typeof request.query.modes === 'string' ? request.query.modes.split(',') : [])
    const modes = (Object.keys(COSTING) as Mode[]).filter(mode => asked.has(mode))
    if (lat == null || !(Math.abs(lat) <= 90) || lng == null || minutes == null || !Number.isInteger(minutes) ||
      minutes < 1 || minutes > MINUTES_MAX || modes.length === 0) {
      return reply.status(400).send({ error: `lat, lng, minutes (1 to ${MINUTES_MAX}) and modes (walk, car) are needed` })
    }
    const answers = await Promise.allSettled(modes.map(async mode => {
      const response = await fetch(`${valhallaUrl}/isochrone`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', ...CLIENT },
        signal: AbortSignal.timeout(ROUTER_TIMEOUT_MS),
        body: JSON.stringify({
          locations: [{ lat, lon: wrappedLongitude(lng) }], costing: COSTING[mode], contours: [{ time: minutes }], polygons: true,
        }),
      })
      const area = response.ok ? (await response.json() as { features?: { geometry?: unknown }[] }).features?.[0]?.geometry : null
      if (!area) throw new Error(`the router answered ${response.status} for ${mode}`)
      return { type: 'Feature', geometry: area, properties: { mode, minutes } }
    }))
    const features = answers.flatMap(answer => answer.status === 'fulfilled' ? [answer.value] : [])
    for (const answer of answers) if (answer.status === 'rejected') request.log.warn({ err: answer.reason }, 'isochron')
    if (features.length === 0) return reply.status(502).send({ error: 'The routing service could not draw the area. Try again in a minute.' })
    return { type: 'FeatureCollection', features }
  })
}
