import { FastifyInstance } from 'fastify'
import { EXPENSIVE_ROUTE_RATE_LIMIT } from '../rate-limit.js'

// The isochrone router backend: VALHALLA_URL names the deployment's own Valhalla (the
// operator builds planet tiles from the extraction PBF); without it the route falls back to
// the public FOSSGIS demo, which answered 502 for weeks in 2026. Read per request (not at
// import) so tests and config reloads take effect.
const valhallaUrl = () => process.env.VALHALLA_URL || 'https://valhalla1.openstreetmap.de'
const VALHALLA_TIMEOUT_MS = 15_000

const MODE_TO_COSTING: Record<string, string> = {
  walk: 'pedestrian',
  car: 'auto',
}

export async function isochronRoutes(app: FastifyInstance) {
  app.get<{ Querystring: { lat: string; lng: string; time: string; modes: string } }>(
    '/api/isochron',
    { config: { rateLimit: EXPENSIVE_ROUTE_RATE_LIMIT } },
    async (request, reply) => {
      const { lat, lng, time, modes } = request.query
      if (!lat || !lng || !time || !modes) {
        return reply.status(400).send({ error: 'Missing: lat, lng, time, modes' })
      }

      const latNum = parseFloat(lat)
      const lngNum = parseFloat(lng)
      const timeNum = parseInt(time, 10)
      if (isNaN(latNum) || isNaN(lngNum) || isNaN(timeNum) || timeNum <= 0) {
        return reply.status(400).send({ error: 'Invalid parameter values' })
      }

      const modeList = modes.split(',').map(m => m.trim()).filter(m => MODE_TO_COSTING[m])
      if (modeList.length === 0) {
        return reply.status(400).send({ error: 'No valid modes. Use: walk, car' })
      }

      try {
        const results = await Promise.all(
          modeList.map(async (mode) => {
            const res = await fetch(`${valhallaUrl()}/isochrone`, {
              method: 'POST',
              headers: { 'Content-Type': 'application/json' },
              signal: AbortSignal.timeout(VALHALLA_TIMEOUT_MS),
              body: JSON.stringify({
                locations: [{ lat: latNum, lon: lngNum }],
                costing: MODE_TO_COSTING[mode],
                contours: [{ time: timeNum }],
                polygons: true,
              }),
            })
            if (!res.ok) throw new Error(`Valhalla ${mode}: ${res.status}`)
            const data = await res.json()
            return data.features[0]
          })
        )

        // Return the largest polygon
        const valid = results.filter(f => f != null)
        if (valid.length === 0) {
          return reply.status(502).send({ error: 'Valhalla returned no features' })
        }
        let largest = valid[0]
        let maxCoords = countCoords(largest)
        for (let i = 1; i < valid.length; i++) {
          const c = countCoords(valid[i])
          if (c > maxCoords) { largest = valid[i]; maxCoords = c }
        }

        return { ...largest, properties: { ...largest.properties, modes: modeList, time: timeNum } }
      } catch (err) {
        return reply.status(502).send({ error: (err as Error).message })
      }
    }
  )
}

function countCoords(feature: any): number {
  const geom = feature?.geometry
  if (!geom) return 0
  if (geom.type === 'Polygon') return geom.coordinates[0]?.length || 0
  if (geom.type === 'MultiPolygon') return geom.coordinates.reduce((sum: number, poly: any) => sum + (poly[0]?.length || 0), 0)
  return 0
}
