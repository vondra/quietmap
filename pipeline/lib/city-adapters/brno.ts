/**
 * Brno — BKOM pentlogram (traffic-flow map) adapter.
 *
 * Source: data.brno.cz ArcGIS item "Intenzita dopravy – Intenzita vozidel"
 * (Brněnské komunikace a.s.), layer `intenzita_dopravy_pentlogramy_lines`:
 * 589 section polylines over the street graph with per-edition columns
 * `car_YYYY` / `truc_YYYY` (2010→2023 today). Per the dataset description,
 * `car` = motor vehicles per 24 h in THOUSANDS and `truc` = PERCENT share
 * of trucks+buses — so total = car×1000, heavy = total×truc/100,
 * light = total − heavy. Buses are inside the truck share and not
 * separable → medium = 0 (folding them into heavy is the conservative
 * CNOSSOS choice); moto not published → 0.
 *
 * Values are section totals over both directions (the 81k peak matches
 * D1's bidirectional volume, not a one-way half). No street names in the
 * data → records carry the section polyline and the driver matches rows
 * by proximity (row start/mid/end must all hug the line).
 *
 * Registry marks this `derived`: BKOM's pentlogram is the city's official
 * measured-network product, but values are working-day-period intensities
 * quantized to whole thousands, with a percent-quantized heavy share, and
 * each edition carries sections forward between survey campaigns — counted
 * and modelled sections are indistinguishable per row.
 */
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { execFileSync } from 'node:child_process'
import { replaceCacheFileAtomically } from '../atomic-cache.js'
import type { CityRecord } from './types.js'
import { DATA_YEAR as YEAR } from '../data-year.js'

const CACHE_DIR = resolve(import.meta.dirname, '..', '..', '..', 'data', 'enrichment', YEAR, 'cz', 'city-brno')
const QUERY_URL =
  'https://services6.arcgis.com/fUWVlHWZNxUvTUh8/arcgis/rest/services/intenzita_dopravy_pentlogramy/FeatureServer/0/query' +
  '?where=1%3D1&outFields=*&outSR=4326&f=geojson'
const RAW_PATH = resolve(CACHE_DIR, 'pentlogram-lines.geojson')
const PARSED_PATH = resolve(CACHE_DIR, 'pentlogram-parsed-v1.json')

/** Plan §2.6 rule 4 — a COVID-edition pentlogram must not become the AADT. */
const EXCLUDED_YEARS = new Set([2020, 2021])
const MIN_FEATURES = 500
const MAX_FEATURES = 1200

type Feature = {
  properties: Record<string, unknown>
  geometry: { type: string; coordinates: unknown }
}

export async function loadBrno(opts: { enrichOnly: boolean; forceDownload: boolean }): Promise<CityRecord[]> {
  mkdirSync(CACHE_DIR, { recursive: true })
  if (existsSync(PARSED_PATH) && !opts.forceDownload) {
    const cached: { year: number; records: CityRecord[] } = JSON.parse(readFileSync(PARSED_PATH, 'utf8'))
    console.log(`[brno] cache: pentlogram ${cached.year}, ${cached.records.length} sections`)
    return cached.records
  }
  if (!existsSync(RAW_PATH) || opts.forceDownload) {
    if (opts.enrichOnly) throw new Error(`brno adapter: --enrich-only but no cache at ${RAW_PATH}`)
    replaceCacheFileAtomically(RAW_PATH, temporaryPath => {
      execFileSync('curl', ['-fsSL', '--max-time', '300', QUERY_URL, '-o', temporaryPath])
    })
  }

  const fc = JSON.parse(readFileSync(RAW_PATH, 'utf8'))
  if (fc.exceededTransferLimit === true || fc.properties?.exceededTransferLimit === true) {
    throw new Error('brno adapter: FeatureServer page limit hit — layer outgrew a single query, add paging')
  }
  const features: Feature[] = fc.features ?? []
  if (features.length < MIN_FEATURES || features.length > MAX_FEATURES) {
    throw new Error(`brno adapter: ${features.length} features (expected ~589) — service drift?`)
  }

  // Latest usable pentlogram edition: newest car_YYYY column that is
  // populated on ≥90 % of sections (a half-filled in-progress edition
  // must not win) and is not a COVID year.
  const editionYears = [...new Set(
    features.flatMap((f) => Object.keys(f.properties).map((k) => /^car_(\d{4})$/.exec(k)?.[1]).filter((y): y is string => y !== undefined)),
  )].map(Number).filter((y) => !EXCLUDED_YEARS.has(y)).sort((a, b) => b - a)
  const populated = (y: number): number => features.filter((f) => ((f.properties[`car_${y}`] as number) ?? 0) > 0).length
  const usedYear = editionYears.find((y) => populated(y) >= features.length * 0.9)
  if (usedYear === undefined) {
    throw new Error(`brno adapter: no edition year is ≥90 % populated (years: ${editionYears.join(',')})`)
  }

  const records: CityRecord[] = []
  for (const f of features) {
    const carThousands = f.properties[`car_${usedYear}`] as number | null
    const trucPct = (f.properties[`truc_${usedYear}`] as number | null) ?? 0
    if (carThousands == null || carThousands <= 0) continue
    // Physical bounds, not typicality: truc is a 0–100 % share (two real
    // freight-access sections sit at 60/90 %), car beyond 150k veh/day
    // exceeds any Czech road and would mean the "thousands" unit drifted.
    if (carThousands > 150 || trucPct < 0 || trucPct > 100) {
      throw new Error(`brno adapter: implausible section (car=${carThousands}k, truc=${trucPct}%) — unit drift?`)
    }
    const total = carThousands * 1000
    const heavy = Math.round((total * trucPct) / 100)
    const label = `BKOM section ${f.properties.id ?? f.properties.ObjectId}`
    // One record per polyline part: a MultiLineString flattened into one
    // line would grow a phantom connector segment that proximity-matches
    // unrelated rows across the gap.
    const parts: ReadonlyArray<ReadonlyArray<readonly [number, number]>> =
      f.geometry.type === 'LineString'
        ? [f.geometry.coordinates as [number, number][]]
        : f.geometry.type === 'MultiLineString'
          ? (f.geometry.coordinates as [number, number][][])
          : []
    if (parts.length === 0) throw new Error(`brno adapter: unexpected geometry ${f.geometry.type}`)
    for (const line of parts) {
      records.push({
        street: label,
        aadtLight: total - heavy,
        aadtMedium: 0,
        aadtHeavy: heavy,
        aadtMoto: 0,
        line,
      })
    }
  }
  if (records.length < MIN_FEATURES) {
    throw new Error(`brno adapter: only ${records.length} usable sections in edition ${usedYear}`)
  }

  replaceCacheFileAtomically(PARSED_PATH, temporaryPath => {
    writeFileSync(temporaryPath, JSON.stringify({ year: usedYear, records }))
  })
  console.log(`[brno] pentlogram ${usedYear}: ${records.length} section records (cache ${PARSED_PATH})`)
  return records
}
