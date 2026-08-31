import type { FastifyInstance } from 'fastify'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'

/**
 * Server-side proxy + disk cache for hexdb.io aircraft lookups.
 *
 * The popup needs aircraft metadata (registration, type, operator, model
 * name) for ICAO 24-bit hex addresses surfaced via packed `flight_id`.
 * Hitting hexdb.io directly from the browser works (CORS-friendly), but
 * burns rate limit when many popup viewers click on the same well-trafficked
 * airliner — proxying through Fastify lets us cache responses on disk and
 * reuse across users without leaking client IPs.
 *
 * Cache strategy: filesystem JSON files keyed by lowercase 6-char ICAO hex,
 * laid out 2-deep (`<prefix>/<hex>.json`) to keep directory size bounded
 * (~256 dirs × ~2300 entries when full). Cache hits are sync read; misses
 * fetch hexdb.io with a 5-second timeout and write through. Negative
 * results (404 / unknown ICAO) are also cached — a permanently anonymous
 * transponder shouldn't repeatedly hit hexdb.
 *
 * The popup tooltip + click-to-info feature requires these lookups;
 * the disk cache is what keeps the worst-case popup latency bounded
 * when 10+ flights show up in one query.
 */

const CACHE_ROOT = process.env.AIRCRAFT_CACHE_DIR ?? join(tmpdir(), 'quietmap-aircraft-cache')
const HEXDB_BASE = 'https://hexdb.io/api/v1/aircraft'
const REQUEST_TIMEOUT_MS = 5_000

const HEX_RE = /^[0-9a-f]{6}$/

interface AircraftInfo {
  hex: string
  registration: string | null
  icao_typecode: string | null
  manufacturer: string | null
  type: string | null
  operator: string | null
  operator_icao: string | null
  /** Source of truth for the cached entry — `hexdb` for normal hits,
   *  `unknown` for 404s. Lets clients distinguish "we don't know" from
   *  "we never asked". */
  source: 'hexdb' | 'unknown'
  /** Unix milliseconds the cache entry was written. Frontend can decide
   *  to ignore stale entries client-side, but the disk cache itself is
   *  immutable across server restarts. */
  fetched_at: number
}

function cachePath(hex: string): string {
  return join(CACHE_ROOT, hex.slice(0, 2), `${hex}.json`)
}

async function readCache(hex: string): Promise<AircraftInfo | null> {
  try {
    const buf = await readFile(cachePath(hex), 'utf8')
    return JSON.parse(buf) as AircraftInfo
  } catch {
    return null
  }
}

async function writeCache(info: AircraftInfo): Promise<void> {
  const path = cachePath(info.hex)
  await mkdir(dirname(path), { recursive: true })
  await writeFile(path, JSON.stringify(info), 'utf8')
}

async function fetchHexdb(hex: string): Promise<AircraftInfo> {
  const ctrl = new AbortController()
  const timer = setTimeout(() => ctrl.abort(), REQUEST_TIMEOUT_MS)
  try {
    const r = await fetch(`${HEXDB_BASE}/${hex.toUpperCase()}`, { signal: ctrl.signal })
    if (r.status === 404) {
      return {
        hex,
        registration: null,
        icao_typecode: null,
        manufacturer: null,
        type: null,
        operator: null,
        operator_icao: null,
        source: 'unknown',
        fetched_at: Date.now(),
      }
    }
    if (!r.ok) throw new Error(`hexdb HTTP ${r.status}`)
    const body = (await r.json()) as Record<string, unknown>
    return {
      hex,
      registration: typeof body.Registration === 'string' ? body.Registration : null,
      icao_typecode: typeof body.ICAOTypeCode === 'string' ? body.ICAOTypeCode : null,
      manufacturer: typeof body.Manufacturer === 'string' ? body.Manufacturer : null,
      type: typeof body.Type === 'string' ? body.Type : null,
      operator: typeof body.RegisteredOwners === 'string' ? body.RegisteredOwners : null,
      operator_icao: typeof body.OperatorFlagCode === 'string' ? body.OperatorFlagCode : null,
      source: 'hexdb',
      fetched_at: Date.now(),
    }
  } finally {
    clearTimeout(timer)
  }
}

export async function aircraftRoutes(app: FastifyInstance): Promise<void> {
  app.get<{ Params: { hex: string } }>('/api/aircraft/:hex', async (req, reply) => {
    const hex = req.params.hex.toLowerCase()
    if (!HEX_RE.test(hex)) {
      reply.code(400)
      return { error: 'invalid ICAO hex (expected 6 hex chars)' }
    }
    const cached = await readCache(hex)
    if (cached) return cached
    try {
      const fresh = await fetchHexdb(hex)
      await writeCache(fresh).catch(() => {
        // Disk cache write failures are non-fatal — return the fresh
        // payload anyway so the user gets the lookup result. The next
        // request will retry the cache write.
      })
      return fresh
    } catch (e) {
      reply.code(502)
      return { error: 'hexdb fetch failed', detail: (e as Error).message }
    }
  })
}
