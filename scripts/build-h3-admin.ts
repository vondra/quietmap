/**
 * Write each prepared H3R4 cell's admin record
 * (`data/prepared/{YEAR}/h3r4/<cell>/admin.bin`).
 *
 * For every land H3R4 hex present in `data/prepared/{YEAR}/h3r4/`, assign
 * (continent, country, metro_city) and store it beside that cell's arrows, so
 * a paint task reads admin for exactly its own read ring and nothing
 * world-wide has to travel with it. The engine reads a cell's record on first
 * use and selects per-country / per-city defaults in the hierarchical traffic
 * cascade (see `engine/noise-compute/src/defaults.rs`).
 *
 * Resolution (plan M2 §1 — ONE polygon authority): AdminAt, the global CGAZ
 * point resolver (`pipeline/lib/admin-at.ts`). Natural Earth is RETIRED — its
 * generalization mis-assigned multi-km border salients (Hlučínsko; see
 * pipeline/lib/country-polygon.ts) and its centroid-only rule left every
 * sea-centroid coastal/island hex UNKNOWN (the Koh Phangan WORLD-defaults
 * defect). Per hex:
 *
 *   1. centroid PIP via adminAt;
 *   2. if undefined (centroid over water), 37-point interior sampling
 *      (centroid + 6 vertices + 6 edge midpoints + 24 inner points,
 *      antimeridian-safe interpolation) and the MAX-SHARE ISO wins — never
 *      first-hit, so a mostly-Thai island hex can no longer fall to WORLD
 *      because a sample clipped a neighbour's polygon first;
 *   3. still undefined → true ocean hex (e.g. a lighthouse building): iso
 *      "\0\0", continent UNKNOWN.
 *
 * Metros: centroid PIP via cityAt, gated by the RESOLVED country (a metro
 * rectangle assigns only when the hex's country matches the metro's own).
 *
 * Geopolitical note: CGAZ ADM0 encodes its own view of contested boundaries
 * (US-DoS disputed-area codes carry no ISO identity). The project
 * policy-maps the three road-bearing groups to their administering country
 * (pipeline/lib/admin-at.ts DISPUTED_SHAPEGROUP_ISO: Falklands → FK, Aksai
 * Chin → CN, Abyei → SD); any other disputed land stays UNKNOWN. This is a
 * documented project policy: the assignment is a best-effort approximation,
 * regenerable without touching arrow data.
 *
 * Output record (13 bytes, little-endian), one file per cell:
 *   [u64 hex_id, u8 continent, u8 country, u16 city]
 * The hex id repeats the directory name so a record copied into another cell
 * is caught by the reader (engine/noise-compute/src/admin.rs) instead of
 * believed. Additive and idempotent: it replaces each cell's own record by
 * rename and touches no other file.
 *
 * Usage:
 *   cd scripts && npm i    # one-time (needs tsx)
 *   DATA_YEAR=2026 npx tsx build-h3-admin.ts
 */

import { readFileSync, writeFileSync, renameSync, existsSync, readdirSync } from 'node:fs'
import { resolve, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { cellToLatLng, cellToBoundary } from 'h3-js'
import { adminAt, antimeridianLerp, cityAt, continentForIso } from '../pipeline/lib/admin-at.js'

const __dirname = dirname(fileURLToPath(import.meta.url))
const YEAR = process.env.DATA_YEAR || JSON.parse(readFileSync(resolve(__dirname, 'dataset-year.json'), 'utf8')).current_year
const H3R4_DIR = resolve(__dirname, `../data/prepared/${YEAR}/h3r4`)
/** Mirrors engine/noise-compute/src/admin.rs::ADMIN_FILE_NAME and RECORD_SIZE. */
const ADMIN_FILE_NAME = 'admin.bin'
const RECORD_SIZE = 8 + 1 + 2 + 2

// ─── Continent ids (mirror engine/noise-compute/src/admin.rs::Continent) ────

const CONTINENT_ID = {
  UNKNOWN: 0,
  Europe: 1,
  NorthAmerica: 2,
  SouthAmerica: 3,
  Asia: 4,
  Africa: 5,
  Oceania: 6,
} as Record<string, number>

function continentIdForIso(iso: string): number {
  return CONTINENT_ID[continentForIso(iso) ?? ''] ?? CONTINENT_ID.UNKNOWN
}

// ─── Hex enumeration ───────────────────────────────────────────────────────

/** All valid H3R4 hex directories in `data/prepared/{YEAR}/h3r4/`. */
function landHexes(): string[] {
  if (!existsSync(H3R4_DIR)) {
    throw new Error(`H3R4 dir not found: ${H3R4_DIR}`)
  }
  const entries = readdirSync(H3R4_DIR)
  // H3 res-4 id format: 15 hex chars. Layout: "84" (2) + 5 hex digits of
  // base-cell + direction path + "ffffffff" (8) padding for unused digits.
  return entries.filter((e) => /^84[0-9a-f]{5}ffffffff$/.test(e)).sort()
}

// ─── Interior sampling (sea-centroid hexes) ────────────────────────────────

/** 37 sample points across the hex: centroid + 6 vertices + 6 edge midpoints
 *  + 24 inner points (⅓/⅔ along centroid→vertex and centroid→edge-midpoint).
 *  The 12 res-4 PENTAGON cells yield 31 (5 vertices). Longitudes interpolate
 *  the short way around the globe (hexes centred near ±180° have vertices on
 *  both sides — naive averaging would sample ~0°). */
function hexSamples(hexStr: string): [number, number][] {
  const [cLat, cLon] = cellToLatLng(hexStr)
  const boundary = cellToBoundary(hexStr) // [[lat, lon] × 6, ×5 for pentagons]
  const n = boundary.length
  const pts: [number, number][] = [[cLat, cLon]]
  const edgeMids: [number, number][] = []
  for (let i = 0; i < n; i++) {
    const [vLat, vLon] = boundary[i]
    const [nLat, nLon] = boundary[(i + 1) % n]
    pts.push([vLat, vLon])
    const mid = antimeridianLerp(vLat, vLon, nLat, nLon, 0.5)
    edgeMids.push(mid)
    pts.push(mid)
  }
  for (let i = 0; i < n; i++) {
    const [vLat, vLon] = boundary[i]
    for (const t of [1 / 3, 2 / 3]) pts.push(antimeridianLerp(cLat, cLon, vLat, vLon, t))
    const [mLat, mLon] = edgeMids[i]
    for (const t of [1 / 3, 2 / 3]) pts.push(antimeridianLerp(cLat, cLon, mLat, mLon, t))
  }
  return pts // 1 + n + n + 2n + 2n (n = 6 → 37, n = 5 → 31)
}

/** Max-share ISO over the interior samples; undefined when no sample resolves
 *  (true ocean). Ties break lexicographically — deterministic; real coastal
 *  hexes have a clear winner. */
function maxShareIso(hexStr: string): string | undefined {
  const shares = new Map<string, number>()
  for (const [sLat, sLon] of hexSamples(hexStr)) {
    const iso = adminAt(sLat, sLon).iso2
    if (iso !== undefined) shares.set(iso, (shares.get(iso) ?? 0) + 1)
  }
  let best: string | undefined
  let bestN = 0
  for (const [iso, n] of [...shares.entries()].sort()) {
    if (n > bestN) {
      best = iso
      bestN = n
    }
  }
  return best
}

// ─── Main ──────────────────────────────────────────────────────────────────

async function main() {
  console.log('Enumerating land hexes from arrow data...')
  const hexes = landHexes()
  console.log(`  ${hexes.length} H3R4 land hexes`)

  console.log('Assigning admin per hex (AdminAt centroid, max-share fallback)...')
  const t0 = Date.now()
  let processed = 0
  let centroidResolved = 0
  let sampledResolved = 0
  const stillUnknown: string[] = []
  const records: {
    hexStr: string
    continent: number
    iso: string        // two-letter or "" if unknown
    city: number       // 0 = none
  }[] = []

  for (const hexStr of hexes) {
    const [lat, lon] = cellToLatLng(hexStr)

    let iso = adminAt(lat, lon).iso2
    if (iso !== undefined) {
      centroidResolved++
    } else {
      iso = maxShareIso(hexStr)
      if (iso !== undefined) sampledResolved++
      else stillUnknown.push(hexStr)
    }
    const continent = iso !== undefined ? continentIdForIso(iso) : CONTINENT_ID.UNKNOWN
    const city = cityAt(lat, lon, iso)

    records.push({ hexStr, continent, iso: iso ?? '', city })

    if (++processed % 10_000 === 0) {
      const dt = ((Date.now() - t0) / 1000).toFixed(0)
      console.log(`  [${dt}s] ${processed}/${hexes.length}`)
    }
  }
  console.log(`  done in ${((Date.now() - t0) / 1000).toFixed(0)}s`)
  console.log(`  centroid-resolved: ${centroidResolved}, sample-resolved: ${sampledResolved}, still UNKNOWN: ${stillUnknown.length}`)
  if (stillUnknown.length > 0) {
    console.log(`  UNKNOWN hexes (first 50): ${stillUnknown.slice(0, 50).join(' ')}`)
  }

  // ─── Per-cell serialization ─────────────────────────────────────────────
  // One 13-byte record per cell: u64 hex_id + u8 continent + [u8; 2] iso +
  // u16 city. ISO bytes are ASCII (A-Z); "\0\0" means unknown. No ID
  // allocation needed — defaults.rs matches on iso directly. Written to a
  // sibling temp name and renamed, so a reader mid-run sees either the whole
  // old record or the whole new one, never a torn file.
  const buf = Buffer.alloc(RECORD_SIZE)
  for (const r of records) {
    // The H3 id is a 64-bit integer written as a 15-char hex string (60 bits,
    // top nibble always 0); BigInt sidesteps JS's 32-bit bit operations.
    buf.writeBigUInt64LE(BigInt('0x' + r.hexStr), 0)
    buf.writeUInt8(r.continent, 8)
    buf.writeUInt8(r.iso.charCodeAt(0) || 0, 9)
    buf.writeUInt8(r.iso.charCodeAt(1) || 0, 10)
    buf.writeUInt16LE(r.city, 11)
    const path = resolve(H3R4_DIR, r.hexStr, ADMIN_FILE_NAME)
    const temporary = `${path}.tmp`
    writeFileSync(temporary, buf)
    renameSync(temporary, path)
  }
  console.log(
    `✓ wrote ${records.length} × ${ADMIN_FILE_NAME} (${RECORD_SIZE} B each) under ${H3R4_DIR}`,
  )

  // ─── Distribution summary ───────────────────────────────────────────────
  const byContinent = new Map<number, number>()
  const byIso = new Map<string, number>()
  let cityCount = 0
  for (const r of records) {
    byContinent.set(r.continent, (byContinent.get(r.continent) || 0) + 1)
    byIso.set(r.iso, (byIso.get(r.iso) || 0) + 1)
    if (r.city > 0) cityCount++
  }
  console.log('\nDistribution:')
  console.log(`  continents: ${[...byContinent.entries()].map(([c, n]) => `${c}=${n}`).join(', ')}`)
  console.log(`  unique countries: ${byIso.size - (byIso.has('') ? 1 : 0)}`)
  console.log(`  unknown country: ${byIso.get('') || 0} hexes`)
  console.log(`  in a metro: ${cityCount} hexes`)
}

main().catch((e) => {
  console.error(e)
  process.exit(1)
})
