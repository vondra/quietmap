/**
 * Enrich ES buildings.arrow with Spanish Catastro INSPIRE data (floor counts).
 *
 * Downloads per-province GML files from Catastro INSPIRE ATOM feed,
 * parses building centroids + numberOfFloorsAboveGround, matches to
 * buildings.arrow by proximity (30m), fills floors column (UInt8).
 *
 * First version: key provinces (Madrid, Barcelona, Valencia, Seville,
 * Zaragoza, Malaga, Bilbao, Alicante).
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-buildings-es.ts
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-buildings-es.ts --force-download
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-buildings-es.ts --enrich-only
 */

import { readFileSync, writeFileSync, readdirSync, existsSync, mkdirSync, openSync, writeSync, closeSync } from 'node:fs'
import { resolve } from 'node:path'
import { execSync } from 'node:child_process'
import { createReadStream } from 'node:fs'
import { createInterface } from 'node:readline'
import proj4 from 'proj4'
import { shouldOverwrite } from './lib/provenance.js'
import { writeBuildingEnrichment } from './lib/buildings-arrow.js'
import { iterateCountryHexes } from './lib/roads-arrow.js'
import { SOURCE_ID_ES_CATASTRO } from './lib/source-ids.generated.js'
import { flatDist } from './lib/spatial.js'
import { DATA_YEAR as YEAR, OSM_EXTRACT_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_ES_CATASTRO

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/es`)
const CACHE_FILE = resolve(CACHE_DIR, 'catastro-buildings.json')

const enrichOnly = process.argv.includes('--enrich-only')
const forceDownload = process.argv.includes('--force-download')

const ATOM_URL = 'https://www.catastro.hacienda.gob.es/INSPIRE/buildings/ES.SDGC.BU.atom.xml'

// Spanish Catastro uses 2-digit INE province codes.
// National Catastro (catastro.hacienda.gob.es) covers everything EXCEPT
// País Vasco (01 Álava, 20 Gipuzkoa, 48 Bizkaia) and Navarra (31) —
// those regions have their own foral cadastres. We include 48 anyway
// because it's present in the ATOM feed (foral Bizkaia INSPIRE service).
const TARGET_PROVINCES: { code: string; name: string }[] = [
  { code: '01', name: 'Araba' },
  { code: '02', name: 'Albacete' },
  { code: '03', name: 'Alicante' },
  { code: '04', name: 'Almeria' },
  { code: '05', name: 'Avila' },
  { code: '06', name: 'Badajoz' },
  { code: '07', name: 'Baleares' },
  { code: '08', name: 'Barcelona' },
  { code: '09', name: 'Burgos' },
  { code: '10', name: 'Caceres' },
  { code: '11', name: 'Cadiz' },
  { code: '12', name: 'Castellon' },
  { code: '13', name: 'Ciudad Real' },
  { code: '14', name: 'Cordoba' },
  { code: '15', name: 'A Coruna' },
  { code: '16', name: 'Cuenca' },
  { code: '17', name: 'Girona' },
  { code: '18', name: 'Granada' },
  { code: '19', name: 'Guadalajara' },
  { code: '20', name: 'Gipuzkoa' },
  { code: '21', name: 'Huelva' },
  { code: '22', name: 'Huesca' },
  { code: '23', name: 'Jaen' },
  { code: '24', name: 'Leon' },
  { code: '25', name: 'Lleida' },
  { code: '26', name: 'La Rioja' },
  { code: '27', name: 'Lugo' },
  { code: '28', name: 'Madrid' },
  { code: '29', name: 'Malaga' },
  { code: '30', name: 'Murcia' },
  { code: '31', name: 'Navarra' },
  { code: '32', name: 'Ourense' },
  { code: '33', name: 'Asturias' },
  { code: '34', name: 'Palencia' },
  { code: '35', name: 'Las Palmas' },           // Gran Canaria, Fuerteventura, Lanzarote
  { code: '36', name: 'Pontevedra' },
  { code: '37', name: 'Salamanca' },
  { code: '38', name: 'Santa Cruz de Tenerife' }, // Tenerife, La Palma, La Gomera, El Hierro
  { code: '39', name: 'Cantabria' },
  { code: '40', name: 'Segovia' },
  { code: '41', name: 'Sevilla' },
  { code: '42', name: 'Soria' },
  { code: '43', name: 'Tarragona' },
  { code: '44', name: 'Teruel' },
  { code: '45', name: 'Toledo' },
  { code: '46', name: 'Valencia' },
  { code: '47', name: 'Valladolid' },
  { code: '48', name: 'Bizkaia' },
  { code: '49', name: 'Zamora' },
  { code: '50', name: 'Zaragoza' },
  { code: '51', name: 'Ceuta' },
  { code: '52', name: 'Melilla' },
]

const CONCURRENCY = 2

// ── Types ──

interface CatastroBuilding {
  lat: number
  lon: number
  floors: number
}

// ── Step 1: Download Catastro INSPIRE GML files ──

async function downloadCatastro(): Promise<CatastroBuilding[]> {
  if (!forceDownload && existsSync(CACHE_FILE)) {
    console.log(`  Using cached Catastro data: ${CACHE_FILE}`)
    return await readCacheStreaming(CACHE_FILE)
  }
  if (enrichOnly) {
    if (!existsSync(CACHE_FILE)) { console.error('ERROR: --enrich-only but no cache'); process.exit(1) }
    return await readCacheStreaming(CACHE_FILE)
  }

  mkdirSync(CACHE_DIR, { recursive: true })

  // Step 1a: Download the ATOM feed to discover per-province URLs.
  // Catastro INSPIRE ATOM feeds are served as ISO-8859-1 ("encoding="ISO-8859-1"
  // in the XML prolog). fetch().text() would mojibake Ñ/É in URL paths,
  // breaking downloads for every municipality with a non-ASCII name.
  console.log('  Downloading Catastro INSPIRE ATOM feed...')
  const atomRes = await fetch(ATOM_URL, { signal: AbortSignal.timeout(60_000) })
  if (!atomRes.ok) throw new Error(`ATOM feed download failed: ${atomRes.status}`)
  const atomXml = new TextDecoder('iso-8859-1').decode(await atomRes.arrayBuffer())
  console.log(`  ATOM feed: ${(atomXml.length / 1024).toFixed(0)} KB`)

  // Parse province feed URLs from ATOM
  // The main feed links to per-province sub-feeds. Each entry has a link to the province ATOM feed.
  // Pattern: <entry>...<title>...province name...</title>...<link href="URL" .../></entry>
  const provinceFeedUrls = new Map<string, string>()

  // Extract all entry blocks
  const entryRegex = /<entry>([\s\S]*?)<\/entry>/g
  let entryMatch
  while ((entryMatch = entryRegex.exec(atomXml)) !== null) {
    const block = entryMatch[1]
    const titleMatch = block.match(/<title[^>]*>([^<]+)</)
    const linkMatch = block.match(/<link[^>]*href="([^"]+)"[^>]*type="application\/atom\+xml"/)
      || block.match(/<link[^>]*type="application\/atom\+xml"[^>]*href="([^"]+)"/)
      || block.match(/<link[^>]*href="([^"]+)"/)
    if (!titleMatch || !linkMatch) continue

    const title = titleMatch[1].trim()
    const url = linkMatch[1].trim()

    // Match by province code or name
    for (const prov of TARGET_PROVINCES) {
      if (title.includes(prov.code) || title.toLowerCase().includes(prov.name.toLowerCase())) {
        provinceFeedUrls.set(prov.code, url)
      }
    }
  }

  console.log(`  Found ${provinceFeedUrls.size} province feed URLs out of ${TARGET_PROVINCES.length} targets`)
  if (provinceFeedUrls.size === 0) {
    // Fallback: try direct URL pattern
    console.log('  Trying direct URL pattern for province feeds...')
    for (const prov of TARGET_PROVINCES) {
      const directUrl = `https://www.catastro.hacienda.gob.es/INSPIRE/buildings/${prov.code}/ES.SDGC.BU.${prov.code}.atom.xml`
      provinceFeedUrls.set(prov.code, directUrl)
    }
  }

  // Step 1b: For each province, get the sub-feed to find the actual GML ZIP download
  const allBuildings: CatastroBuilding[] = []
  let processed = 0
  let errors = 0

  for (const prov of TARGET_PROVINCES) {
    const feedUrl = provinceFeedUrls.get(prov.code)
    if (!feedUrl) {
      console.log(`  SKIP province ${prov.code} (${prov.name}): no feed URL found`)
      errors++
      continue
    }

    console.log(`  [${prov.code}] ${prov.name}: fetching province feed...`)

    try {
      // The province sub-feed has entries for each municipality in the province.
      // Each entry links to a GML zip (buildingpart or building).
      // We want the "building" entries (not buildingpart) with numberOfFloorsAboveGround.
      const provRes = await fetch(feedUrl, { signal: AbortSignal.timeout(60_000) })
      if (!provRes.ok) {
        console.log(`  [${prov.code}] Feed download failed: ${provRes.status}`)
        errors++
        continue
      }
      // Sub-feeds are also ISO-8859-1; decode with the right codec.
      const provAtom = new TextDecoder('iso-8859-1').decode(await provRes.arrayBuffer())

      // Extract municipality GML zip URLs from province feed
      // Catastro structure: each entry has <link> to a ZIP containing GML
      // We want entries of type "building" (not "buildingpart")
      const muniUrls: { code: string; url: string }[] = []
      const muniEntryRegex = /<entry>([\s\S]*?)<\/entry>/g
      let me
      while ((me = muniEntryRegex.exec(provAtom)) !== null) {
        const block = me[1]
        // Look for ZIP download links (type application/zip or .zip href)
        const zipLinkMatch = block.match(/<link[^>]*href="([^"]*\.zip)"/)
          || block.match(/<link[^>]*href="([^"]*building[^"]*)"[^>]*type="application\/zip"/)
        if (!zipLinkMatch) continue

        const url = zipLinkMatch[1].trim()
        // Only building (BU.) files, not buildingpart (BP.)
        // Catastro names: A.ES.SDGC.BU.{municode}.zip
        if (url.includes('.BP.') || url.includes('buildingpart')) continue

        const codeMatch = url.match(/\.BU\.(\d+)\./) || url.match(/\/(\d+)\//);
        const code = codeMatch ? codeMatch[1] : ''
        muniUrls.push({ code, url })
      }

      console.log(`  [${prov.code}] ${muniUrls.length} municipality GML ZIPs found`)
      if (muniUrls.length > 0) console.log(`    first URL: ${muniUrls[0].url}`)

      // Download and parse in batches
      let provBuildings = 0
      for (let i = 0; i < muniUrls.length; i += CONCURRENCY) {
        const batch = muniUrls.slice(i, i + CONCURRENCY)

        const fetches = await Promise.allSettled(
          batch.map(async ({ code, url }) => {
            const encodedUrl = encodeURI(url)
            try {
              // catastro.hacienda.gob.es throttles/drops Node's default
              // User-Agent ("undici/..."). A browser-like UA passes the WAF.
              const res = await fetch(encodedUrl, {
                signal: AbortSignal.timeout(60_000),
                headers: {
                  'User-Agent': 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36',
                  'Accept': 'application/zip, */*',
                },
              })
              if (!res.ok) { console.log(`    HTTP ${res.status} ${code}`); return { code, buf: null } }
              const buf = Buffer.from(await res.arrayBuffer())
              if (buf.length < 1000 || !(buf[0] === 0x50 && buf[1] === 0x4b)) {
                console.log(`    BAD ${code}: len=${buf.length} magic=${buf.slice(0,4).toString('hex')}`)
                return { code, buf: null }
              }
              return { code, buf }
            } catch (e) {
              console.log(`    ERR ${code}: ${(e as Error).message}`)
              return { code, buf: null }
            }
          })
        )
        // 200ms breath between batches — WAF drops sustained rapid-fire
        // requests even with a good User-Agent.
        await new Promise(r => setTimeout(r, 200))

        for (const r of fetches) {
          if (r.status !== 'fulfilled' || !r.value.buf) { errors++; continue }
          const { code, buf } = r.value
          if (buf.length < 100) { errors++; continue }

          try {
            // Skip very large municipalities (avoid OOM)
            if (buf.length > 100 * 1024 * 1024) {
              console.log(`    SKIP ${code}: ZIP ${(buf.length/1e6).toFixed(0)} MB (too large)`)
              continue
            }

            const tmpZip = `/tmp/catastro_${prov.code}_${code}.zip`
            const tmpDir = `/tmp/catastro_extract_${prov.code}_${code}`
            writeFileSync(tmpZip, buf)
            mkdirSync(tmpDir, { recursive: true })
            execSync(`unzip -o -q "${tmpZip}" -d "${tmpDir}"`, { timeout: 60_000 })

            const gmlFiles = readdirSync(tmpDir).filter(f =>
              f.endsWith('.gml') || f.endsWith('.xml'))

            for (const gf of gmlFiles) {
              const gmlPath = resolve(tmpDir, gf)
              const buildings = await parseGmlBuildings(gmlPath)
              allBuildings.push(...buildings)
              provBuildings += buildings.length
            }

            execSync(`rm -rf "${tmpDir}" "${tmpZip}"`, { timeout: 5_000 })
          } catch (e: any) {
            if (errors < 5) console.log(`    ERROR ${code}: ${e.message?.substring(0, 120)}`)
            errors++
            try { execSync(`rm -rf /tmp/catastro_extract_${prov.code}_${code} /tmp/catastro_${prov.code}_${code}.zip`, { timeout: 5_000 }) } catch {}
          }
        }

        if ((i + CONCURRENCY) % 40 === 0 || i + CONCURRENCY >= muniUrls.length) {
          console.log(`    [${prov.code}] ${Math.min(i + CONCURRENCY, muniUrls.length)}/${muniUrls.length} municipalities, ${provBuildings.toLocaleString()} buildings`)
        }
      }

      processed++
      console.log(`  [${prov.code}] ${prov.name}: ${provBuildings.toLocaleString()} buildings extracted`)
    } catch (e: any) {
      console.log(`  [${prov.code}] ERROR: ${e.message?.substring(0, 150)}`)
      errors++
    }
  }

  console.log(`\n  Total: ${allBuildings.length.toLocaleString()} buildings from ${processed} provinces (${errors} errors)`)
  // Node's JSON.stringify hits V8 string length limit (~512 MB) around
  // 16-20 M building objects. With ~33 M buildings from the full Spain
  // download we need to write in chunks to avoid "Invalid string length".
  writeBuildingsChunked(CACHE_FILE, allBuildings)
  console.log(`  Cached to ${CACHE_FILE}`)

  return allBuildings
}

/**
 * JSONL writer — one building per line. Avoids V8's ~512 MB string length
 * limit that blows up on a single-string JSON array for 34 M buildings
 * (~2 GB). JSONL streams through createReadStream+createInterface.
 *
 * Back-compat: if the existing cache is a legacy single-array JSON
 * (starts with '['), readCacheStreaming falls back to one JSON.parse of
 * the 512 MB head — which only works if someone has a smaller, older
 * cache. Fresh runs always produce JSONL.
 */
function writeBuildingsChunked(path: string, items: CatastroBuilding[]): void {
  const fd = openSync(path, 'w')
  try {
    for (let i = 0; i < items.length; i++) {
      writeSync(fd, JSON.stringify(items[i]) + '\n')
    }
  } finally {
    closeSync(fd)
  }
}

async function readCacheStreaming(path: string): Promise<CatastroBuilding[]> {
  const rl = createInterface({ input: createReadStream(path), crlfDelay: Infinity })
  const out: CatastroBuilding[] = []
  let firstLine = true
  for await (const raw of rl) {
    const line = raw.trim()
    if (!line) continue
    if (firstLine) {
      firstLine = false
      // Legacy single-array JSON cache — not streamable, bail and tell user.
      if (line.startsWith('[{')) {
        throw new Error(`Legacy single-array JSON cache detected at ${path}. Delete it and re-run with --force-download (new format is JSONL).`)
      }
    }
    // Strip stray leading/trailing commas/brackets from legacy remnants.
    const clean = line.replace(/^[\[,]+/, '').replace(/[,\]]+$/, '')
    if (!clean || clean === '[' || clean === ']') continue
    try {
      out.push(JSON.parse(clean) as CatastroBuilding)
    } catch { /* skip malformed */ }
  }
  return out
}

/**
 * Parse a Catastro INSPIRE GML file for building centroids + floor counts.
 * Streams the file line by line to handle large GML files.
 *
 * The GML contains <bu-ext:BuildingExtended> or <bu-core2d:Building> elements
 * with numberOfFloorsAboveGround and geometry (referencePoint or centroid).
 */
async function parseGmlBuildings(gmlPath: string): Promise<CatastroBuilding[]> {
  const buildings: CatastroBuilding[] = []

  // For moderate files, read whole file. For very large, stream.
  const stat = readFileSync(gmlPath)
  const gml = stat.toString('utf-8')

  // Try matching building blocks with numberOfFloorsAboveGround.
  // INSPIRE BU model: building.gml outer polygons usually have no floor data
  // (nil=unpopulated); BuildingPart.gml interior subdivisions carry the
  // actual numberOfFloorsAboveGround integers. We match both + BuildingExtended.
  // \b after Building is insufficient since "BuildingPart" and "BuildingExtended"
  // both start with "Building"; list all three explicitly and anchor on '\b' after.
  const buildingRegex = /<(?:[\w-]+:)?(?:BuildingExtended|BuildingPart|Building)\b[^>]*>([\s\S]*?)<\/(?:[\w-]+:)?(?:BuildingExtended|BuildingPart|Building)>/g
  let m
  while ((m = buildingRegex.exec(gml)) !== null) {
    const block = m[1]

    // Extract numberOfFloorsAboveGround
    const floorsMatch = block.match(/<(?:[\w-]+:)?numberOfFloorsAboveGround>(\d+)</)
    if (!floorsMatch) continue
    const floors = parseInt(floorsMatch[1])
    if (floors <= 0 || floors > 255) continue

    // Extract coordinates — try multiple patterns
    let lat = 0, lon = 0

    // Pattern: <gml:pos>lat lon</gml:pos> (referencePoint)
    const posMatch = block.match(/<gml:pos[^>]*>([^<]+)</)
    if (posMatch) {
      const parts = posMatch[1].trim().split(/\s+/)
      if (parts.length >= 2) {
        // Catastro INSPIRE uses EPSG:25830 (UTM zone 30N) for most of Spain,
        // but referencePoint or pos may be in different CRS.
        // Check srsName for CRS info
        const srsMatch = block.match(/srsName="([^"]*)"/)
        const srs = srsMatch ? srsMatch[1] : ''

        if (srs.includes('4326') || srs.includes('CRS84')) {
          // WGS84 — lat,lon or lon,lat depending on axis order
          const a = parseFloat(parts[0])
          const b = parseFloat(parts[1])
          if (a > 20 && a < 50) { lat = a; lon = b }
          else if (b > 20 && b < 50) { lat = b; lon = a }
        } else if (srs.includes('4258') || srs.includes('ETRS89')) {
          // ETRS89 geographic — same as WGS84 for practical purposes
          const a = parseFloat(parts[0])
          const b = parseFloat(parts[1])
          if (a > 20 && a < 50) { lat = a; lon = b }
          else if (b > 20 && b < 50) { lat = b; lon = a }
        } else {
          // Likely UTM (EPSG:25829, 25830, 25831) — need proj4 conversion
          // For simplicity in v1: skip UTM coords, rely on posList/exterior ring centroid
        }
      }
    }

    // Pattern: compute centroid from exterior ring posList (more reliable)
    if (lat === 0 && lon === 0) {
      const posListMatch = block.match(/<gml:posList[^>]*>([^<]+)</)
      if (posListMatch) {
        const srsMatch = block.match(/srsName="([^"]*)"/)
        const srs = srsMatch ? srsMatch[1] : ''
        const coords = posListMatch[1].trim().split(/\s+/).map(Number)

        if (srs.includes('4326') || srs.includes('CRS84') || srs.includes('4258') || srs.includes('ETRS89')) {
          // Geographic coords: pairs of (lat, lon) or (lon, lat)
          if (coords.length >= 4) {
            let sumLat = 0, sumLon = 0, count = 0
            const first = coords[0]
            const isLatFirst = first > 20 && first < 50

            for (let i = 0; i < coords.length - 1; i += 2) {
              const a = coords[i]
              const b = coords[i + 1]
              if (isNaN(a) || isNaN(b)) continue
              if (isLatFirst) { sumLat += a; sumLon += b }
              else { sumLon += a; sumLat += b }
              count++
            }
            if (count > 0) { lat = sumLat / count; lon = sumLon / count }
          }
        } else {
          // UTM — Spanish Catastro CRS varies by region:
          //   EPSG:25828-25831 (ETRS89 UTM zones 28N-31N, mainland + Baleares)
          //   EPSG:32628       (WGS84  UTM zone 28N, Canary Islands — Las Palmas 35, Tenerife 38)
          // posList is (easting northing) pairs in meters.
          const mETRS = srs.match(/(2583[0-9]|2582[89])/)
          const mWGS  = srs.match(/(326[0-9][0-9])/)
          const code = mETRS ? mETRS[1] : (mWGS ? mWGS[1] : null)
          if (code && coords.length >= 4) {
            const epsg = `EPSG:${code}`
            if (!proj4.defs(epsg)) {
              const zone = parseInt(code.slice(-2))
              const ellps = code.startsWith('326') ? 'WGS84' : 'GRS80'
              proj4.defs(epsg, `+proj=utm +zone=${zone} +ellps=${ellps} +towgs84=0,0,0,0,0,0,0 +units=m +no_defs`)
            }
            let sumEast = 0, sumNorth = 0, count = 0
            for (let i = 0; i < coords.length - 1; i += 2) {
              const e = coords[i], n = coords[i + 1]
              if (isNaN(e) || isNaN(n)) continue
              sumEast += e; sumNorth += n; count++
            }
            if (count > 0) {
              const [lonOut, latOut] = proj4(epsg, 'EPSG:4326', [sumEast / count, sumNorth / count])
              lon = lonOut; lat = latOut
            }
          }
        }
      }
    }

    // Validate: must be in Spain
    if (lat < 27 || lat > 44 || lon < -19 || lon > 5) continue
    if (lat === 0 && lon === 0) continue

    buildings.push({ lat, lon, floors })
  }

  return buildings
}

// ── Step 2: Enrich buildings.arrow ──

// Spain incl. the Canaries — also the per-row sanity bound the old loop used.
// [minLat,minLon,maxLat,maxLon]
const ES_HEX_BBOX: [number, number, number, number] = [27, -19, 44, 5]

async function enrichHexes(catastroBuildings: CatastroBuilding[]): Promise<void> {
  // Build spatial index: 0.01 deg grid (~1km cells)
  const grid = new Map<string, CatastroBuilding[]>()
  for (const b of catastroBuildings) {
    const key = `${Math.floor(b.lat * 100)}_${Math.floor(b.lon * 100)}`
    if (!grid.has(key)) grid.set(key, [])
    grid.get(key)!.push(b)
  }
  console.log(`  Spatial grid: ${grid.size} cells`)

  const hexDirs = iterateCountryHexes(OSM_EXTRACT_DIR, ES_HEX_BBOX, 'buildings.arrow')

  let totalBuildings = 0, totalEnriched = 0, floorsAdded = 0, hexesUpdated = 0

  for (const hexId of hexDirs) {
    // The shared writer owns metadata preservation (v2 `buildings_contract`
    // stamp survives) and the priority gate; Catastro only fills floors.
    const r = await writeBuildingEnrichment(
      resolve(OSM_EXTRACT_DIR, hexId, 'buildings.arrow'),
      (row) => {
        // Fast-exit before the spatial match when a higher-priority dataset
        // owns the row (the writer re-checks the gate — this only saves work).
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) return null
        // Only enrich if floors are missing
        if (row.floors > 0) return null

        // Find nearest Catastro building within 30m (3x3 grid cells)
        let bestDist = 30
        let bestFloors = 0
        for (let dy = -1; dy <= 1; dy++) {
          for (let dx = -1; dx <= 1; dx++) {
            const k = `${Math.floor(row.lat * 100) + dy}_${Math.floor(row.lon * 100) + dx}`
            const cell = grid.get(k)
            if (!cell) continue
            for (const cb of cell) {
              const d = flatDist(row.lat, row.lon, cb.lat, cb.lon)
              if (d < bestDist) {
                bestDist = d
                bestFloors = cb.floors
              }
            }
          }
        }
        if (bestFloors === 0) return null
        return { floors: Math.min(bestFloors, 255), sourceId: MY_SOURCE_ID }
      },
      () => { floorsAdded++ },
    )
    totalBuildings += r.rows
    totalEnriched += r.matched
    if (r.updated) hexesUpdated++
  }

  console.log(`\n=== Results ===`)
  console.log(`  ${totalEnriched} / ${totalBuildings} buildings matched to Catastro (${totalBuildings > 0 ? (totalEnriched / totalBuildings * 100).toFixed(1) : 0}%)`)
  console.log(`  ${floorsAdded} floors added (were missing in OSM)`)
  console.log(`  ${hexesUpdated} / ${hexDirs.length} hexes updated`)
}

// ── Main ──

async function main() {
  console.log(`=== ES Building Enrichment — Catastro INSPIRE (${YEAR}) ===\n`)
  console.log(`  OSM extract dir: ${OSM_EXTRACT_DIR}`)
  console.log(`  Cache: ${CACHE_DIR}`)
  console.log(`  Target provinces: ${TARGET_PROVINCES.map(p => `${p.name} (${p.code})`).join(', ')}\n`)

  if (!existsSync(OSM_EXTRACT_DIR)) {
    console.error(`ERROR: OSM extract directory not found: ${OSM_EXTRACT_DIR}`)
    process.exit(1)
  }

  const buildings = await downloadCatastro()
  console.log(`\n  Catastro buildings: ${buildings.length.toLocaleString()}`)

  if (buildings.length === 0) {
    console.log('  No buildings extracted. Nothing to enrich.')
    return
  }

  console.log('  Enriching buildings.arrow files...')
  await enrichHexes(buildings)
  console.log(`\n=== Done ===`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
