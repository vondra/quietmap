/**
 * Enrich FR roads.arrow with Cerema TMJA traffic census data.
 *
 * Downloads the two TMJA CSV releases from data.gouv.fr, reads them through
 * `lib/fr-tmja-census.ts` (AADT class split, Lambert-93→WGS84 endpoints), and
 * matches the sections to OSM roads by route ref + proximity.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-fr.ts
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-fr.ts --enrich-only
 */

import { readFileSync, writeFileSync, existsSync, mkdirSync } from 'node:fs'
import { shouldOverwrite } from './lib/provenance.js'
import { resolve } from 'node:path'
import { parseTmjaFiles, type CensusSection, type TmjaCsvFile } from './lib/fr-tmja-census.js'
import { SOURCE_ID_FR_CEREMA_TMJA } from './lib/source-ids.generated.js'
import { pointToPolylineDist } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_FR_CEREMA_TMJA

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/fr`)
const CACHE_2024 = resolve(CACHE_DIR, 'tmja-2024.csv')
const CACHE_2019 = resolve(CACHE_DIR, 'tmja-2019.csv')
const CACHE_JSON = resolve(CACHE_DIR, 'tmja-census-v3.json')
// v3 (2026-09-06): CRLF-safe header parse, the 2019 ratio_PL encoding and
// section-identity dedup (lib/fr-tmja-census.ts). Every v2 cache carries
// ratio_pl 0.12 on ALL of its sections; the new name IS the invalidation,
// because downloadTmja returns the cache untouched whenever it exists.

const enrichOnly = process.argv.includes('--enrich-only')
const forceDownload = process.argv.includes('--force-download')

const TMJA_2024_URL = 'https://static.data.gouv.fr/resources/trafic-moyen-journalier-annuel-sur-le-reseau-routier-national/20250818-100154/tmja-rrnc-2024.csv'
const TMJA_2019_URL = 'https://static.data.gouv.fr/resources/trafic-moyen-journalier-annuel-sur-le-reseau-routier-national/20211222-165040/tmja-2019.csv'

async function downloadTmja(): Promise<CensusSection[]> {
  if (!forceDownload && existsSync(CACHE_JSON)) {
    console.log(`  Using cached: ${CACHE_JSON}`)
    return JSON.parse(readFileSync(CACHE_JSON, 'utf-8')) as CensusSection[]
  }

  mkdirSync(CACHE_DIR, { recursive: true })

  // Download CSVs
  for (const [url, path, name] of [
    [TMJA_2024_URL, CACHE_2024, 'TMJA 2024 (RRNc)'],
    [TMJA_2019_URL, CACHE_2019, 'TMJA 2019 (full RRN)'],
  ] as const) {
    if (!forceDownload && existsSync(path)) {
      console.log(`  Using cached ${name}`)
      continue
    }
    if (enrichOnly && !existsSync(path)) {
      console.log(`  WARN: ${name} not cached, skipping`)
      continue
    }
    console.log(`  Downloading ${name}...`)
    const res = await fetch(url, { signal: AbortSignal.timeout(60_000) })
    if (!res.ok) throw new Error(`HTTP ${res.status} for ${name}`)
    writeFileSync(path, await res.text())
    console.log(`  Cached ${name}`)
  }

  return parseCsvFiles()
}

function parseCsvFiles(): CensusSection[] {
  // Newest release first — parseTmjaFiles keeps the first record per section.
  const files: TmjaCsvFile[] = []
  for (const [path, label, ratioPlEncoding] of [
    [CACHE_2024, 'TMJA 2024', 'percent'],
    [CACHE_2019, 'TMJA 2019', 'tenths-of-percent-when-integer'],
  ] as const) {
    if (!existsSync(path)) continue
    files.push({ label, csvText: readFileSync(path, 'utf-8'), ratioPlEncoding })
  }

  const { sections, counters } = parseTmjaFiles(files)
  for (const c of counters) {
    console.log(`  ${c.label}: ${c.parsed} sections, ${c.skipped} skipped, ${c.skippedDuplicateSection} already covered by a newer release, ${c.skippedZeroSplit} skipped (rounds to zero AADT split)`)
  }

  writeFileSync(CACHE_JSON, JSON.stringify(sections))
  console.log(`  Cached ${sections.length} sections to ${CACHE_JSON}`)
  return sections
}

// France bbox (+margin); [minLat,minLon,maxLat,maxLon]. iterateCountryHexes skips
// the rest of the planet so the loader doesn't read every roads.arrow on Earth.
const FR_HEX_BBOX: [number, number, number, number] = [41, -5.5, 51.5, 10]

async function enrichArrows(sections: CensusSection[]) {
  const refIndex = new Map<string, CensusSection[]>()
  for (const s of sections) {
    const list = refIndex.get(s.ref) || []
    list.push(s)
    refIndex.set(s.ref, list)
  }
  console.log(`\n  Ref index: ${refIndex.size} unique road refs`)

  const hexDirs = iterateCountryHexes(H3R4_DIR, FR_HEX_BBOX)
  console.log(`  French hexes: ${hexDirs.length}\n`)

  let totalSeg = 0, matched = 0, preserved = 0, hexesUpdated = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hex, 'roads.arrow'),
      (row) => {
        totalSeg++

        // Priority gate: if a higher-priority dataset already owns this row, leave it.
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) {
          if (row.existingSourceId !== 0) preserved++
          return null
        }

        const osmRef = row.ref?.toString().trim() || ''
        // French OSM refs: "A 1", "N 7", "D 906"
        const normRef = osmRef.replace(/\s+/g, '')

        let best: CensusSection | null = null
        let bestDist = Infinity

        if (normRef && refIndex.has(normRef)) {
          for (const c of refIndex.get(normRef)!) {
            const dist = pointToPolylineDist(row.midLat, row.midLon, c.coords)
            if (dist < bestDist) { bestDist = dist; best = c }
          }
        }

        if (!best || bestDist >= 20000) return null  // 20km for ref-matched sections
        return {
          light: best.aadt_light, medium: best.aadt_medium,
          heavy: best.aadt_heavy, moto: best.aadt_moto, sourceId: MY_SOURCE_ID,
        }
      },
      () => { matched++ },
    )
    if (r.updated) hexesUpdated++

    if (hi % 10 === 0) {
      console.log(`  [${Math.round((Date.now() - startTime) / 1000)}s] ${hi + 1}/${hexDirs.length} hexes, ${hexesUpdated} updated, ${matched} matched`)
    }
  }

  console.log(`\n=== Enrichment Results ===`)
  console.log(`  Total segments: ${totalSeg}`)
  console.log(`  Preserved: ${preserved}`)
  console.log(`  Newly matched: ${matched} (${(100 * matched / Math.max(totalSeg, 1)).toFixed(1)}%)`)
  console.log(`  Hexes updated: ${hexesUpdated}`)

  const top = sections.sort((a, b) => b.tmja - a.tmja).slice(0, 10)
  console.log(`\n  Top 10 AADT:`)
  for (const s of top) {
    console.log(`    ${s.ref.padEnd(6)} TMJA=${s.tmja.toLocaleString().padStart(7)} HV=${(s.ratio_pl * 100).toFixed(0)}%  (${s.lat.toFixed(2)}, ${s.lon.toFixed(2)})`)
  }
}

async function main() {
  console.log(`=== FR Road Traffic Enrichment (Cerema TMJA) ===\n`)
  console.log(`  H3R4 dir: ${H3R4_DIR}`)
  console.log(`  Cache: ${CACHE_DIR}\n`)

  const sections = await downloadTmja()
  console.log(`  Total sections: ${sections.length}`)

  const autoroutes = sections.filter(s => s.ref.startsWith('A'))
  const nationales = sections.filter(s => s.ref.startsWith('N'))
  console.log(`  Autoroutes (A): ${autoroutes.length}`)
  console.log(`  Nationales (N): ${nationales.length}`)

  await enrichArrows(sections)
  console.log(`\n=== Done ===`)
}

main().catch(err => { console.error(err); process.exit(1) })
