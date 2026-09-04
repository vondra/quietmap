// GET /api/initial-view — COUNTRY-level first-map-view guess from the client IP.
//
// Feeds the frontend's initial view (frontend utils/initial-view.ts): a
// visitor with no shared #hash link starts at their COUNTRY. We deliberately
// answer country, NOT city: DB-IP City Lite places consumer/mobile ISP pools
// (O2 CZ AS5610, mobile CGNAT…) at the provider's regional registration, not
// the subscriber — measured 2026-07-19 on the owner's O2 line in Kytín, every
// free DB missed the town by 35–230 km while ALL agreed on the country. A
// wrong city zoom (z11) is worse than a right country view, so we only trust
// the reliable signal. Precise location stays a user gesture (locate button).
//
// Data: DB-IP City Lite mmdb (CC-BY 4.0 — attribution lives on the About
// page; refreshed monthly by scripts/update-geoip-db.sh). The database is
// OPTIONAL: absent or unreadable means every response is {source:'none'}
// and the frontend keeps its language fallback — a fresh checkout works
// without it. The lookup is in-memory only; neither the IP nor the result
// is ever stored (About → Privacy documents this).

import { readFile } from 'node:fs/promises'
import { resolve } from 'node:path'
import type { FastifyInstance } from 'fastify'
import { Reader, type CityResponse } from 'mmdb-lib'
import { REPO_ROOT } from '../runtime-paths.js'

export const DEFAULT_GEOIP_DB_PATH = resolve(REPO_ROOT, 'data', 'prepared', 'geoip', 'dbip-city-lite.mmdb')

export type InitialViewResponse = { source: 'ip-country'; country: string } | { source: 'none' }

// Pure record → response mapping, extracted so the contract is unit-testable
// without an mmdb fixture (the database is optional runtime data, not in git).
// The ISO 3166-1 alpha-2 shape is validated with the SAME regex the client uses
// (utils/initial-view.ts fetchIpCountry) so both ends of the wire agree; the
// frontend maps the code to a country centroid+zoom via its COUNTRY_VIEW table.
export function countryFromRecord(record: CityResponse | null): InitialViewResponse {
  const country = record?.country?.iso_code
  if (typeof country !== 'string' || !/^[A-Z]{2}$/.test(country)) {
    return { source: 'none' }
  }
  return { source: 'ip-country', country }
}

export async function initialViewRoutes(
  app: FastifyInstance,
  // Fastify's register() always passes an options OBJECT as the second
  // argument — a bare `databasePath: string` parameter would silently
  // receive `{}` and break the readFile (found live 2026-07-16).
  opts: { databasePath?: string } = {},
): Promise<void> {
  const databasePath = opts.databasePath ?? DEFAULT_GEOIP_DB_PATH
  // One ~130 MB buffer per process, loaded lazily on the first request so a
  // checkout without the database boots identically; `null` = feature off.
  // The PROMISE is memoized (not the result) so N concurrent cold-start
  // requests share one readFile instead of N × 130 MB peaks.
  let readerPromise: Promise<Reader<CityResponse> | null> | undefined
  function getReader(): Promise<Reader<CityResponse> | null> {
    readerPromise ??= readFile(databasePath)
      .then((buffer) => new Reader<CityResponse>(buffer))
      .catch(() => {
        app.log.warn(`initial-view: GeoIP database missing/unreadable at ${databasePath} — serving source:'none'`)
        return null
      })
    return readerPromise
  }

  app.get('/api/initial-view', async (request, reply) => {
    // The answer depends on the caller's IP — never let a shared cache keep it.
    reply.header('Cache-Control', 'private, no-store')
    let record: CityResponse | null = null
    try {
      record = (await getReader())?.get(request.ip) ?? null
    } catch {
      // Unparseable peer address (or a corrupt db node) — same as no match.
    }
    return countryFromRecord(record)
  })
}
