/** Load and match Indonesia's pinned Bina Marga network and observed LHRT values. */

import { loadPinnedRoadLines, buildRoadLineVertexGrid, nearestRoadLine, type PinnedRoadLine } from './pinned-road-lines.js'
import type { RoadLoaderArguments } from './road-loader-cli.js'
import type { RoadRow } from './roads-arrow.js'
import { inBbox } from './spatial.js'

const ID_SCAN_BBOX: [number, number, number, number] = [-11.5, 94.0, 6.5, 141.5]

// Exclusion zones for neighbours
const TIER1_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  // Jabodetabek = Jakarta Metropolitan Area (includes Bogor, Depok, Tangerang, Bekasi)
  { name: 'Jakarta (Jabodetabek)', bbox: [-6.5, 106.4, -6.0, 107.1] },
  { name: 'Surabaya (Gerbangkertosusila)', bbox: [-7.4, 112.5, -7.1, 112.9] },
  { name: 'Bandung (Bandung Raya)', bbox: [-7.1, 107.5, -6.8, 107.8] },
  { name: 'Medan (Mebidang)', bbox: [3.45, 98.55, 3.75, 98.85] },
  { name: 'Semarang', bbox: [-7.05, 110.3, -6.8, 110.55] },
  { name: 'Makassar', bbox: [-5.25, 119.3, -5.05, 119.55] },
  { name: 'Palembang', bbox: [-3.1, 104.6, -2.85, 104.85] },
  { name: 'Denpasar (Sarbagita Bali)', bbox: [-8.85, 115.1, -8.55, 115.35] },
]

// Tier-2 Indonesian cities
const TIER2_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Yogyakarta', bbox: [-7.88, 110.3, -7.72, 110.45] },
  { name: 'Malang', bbox: [-8.05, 112.55, -7.9, 112.7] },
  { name: 'Padang', bbox: [-1.0, 100.28, -0.85, 100.42] },
  { name: 'Pekanbaru', bbox: [0.45, 101.35, 0.6, 101.55] },
  { name: 'Jambi', bbox: [-1.7, 103.55, -1.55, 103.7] },
  { name: 'Bengkulu', bbox: [-3.85, 102.25, -3.7, 102.4] },
  { name: 'Bandar Lampung', bbox: [-5.5, 105.2, -5.3, 105.4] },
  { name: 'Serang', bbox: [-6.15, 106.1, -6.0, 106.25] },
  { name: 'Banjarmasin', bbox: [-3.4, 114.55, -3.25, 114.7] },
  { name: 'Balikpapan', bbox: [-1.3, 116.75, -1.15, 116.95] },
  { name: 'Samarinda', bbox: [-0.55, 117.1, -0.4, 117.25] },
  { name: 'Pontianak', bbox: [-0.1, 109.3, 0.05, 109.4] },
  { name: 'Palangkaraya', bbox: [-2.25, 113.85, -2.1, 114.0] },
  { name: 'Mataram (Lombok)', bbox: [-8.65, 116.05, -8.5, 116.2] },
  { name: 'Kupang (Timor)', bbox: [-10.25, 123.55, -10.1, 123.7] },
  { name: 'Manado', bbox: [1.45, 124.78, 1.58, 124.92] },
  { name: 'Gorontalo', bbox: [0.5, 123.02, 0.62, 123.1] },
  { name: 'Kendari', bbox: [-4.05, 122.5, -3.9, 122.65] },
  { name: 'Palu', bbox: [-0.95, 119.85, -0.8, 120.0] },
  { name: 'Ambon', bbox: [-3.75, 128.15, -3.65, 128.25] },
  { name: 'Jayapura', bbox: [-2.6, 140.65, -2.5, 140.8] },
  { name: 'Ternate', bbox: [0.75, 127.3, 0.85, 127.4] },
  { name: 'Cirebon', bbox: [-6.78, 108.5, -6.68, 108.6] },
  { name: 'Tasikmalaya', bbox: [-7.35, 108.18, -7.25, 108.28] },
  { name: 'Sukabumi', bbox: [-6.98, 106.88, -6.88, 107.0] },
  { name: 'Pekalongan', bbox: [-6.95, 109.63, -6.85, 109.73] },
  { name: 'Tegal', bbox: [-6.9, 109.1, -6.8, 109.2] },
  { name: 'Solo (Surakarta)', bbox: [-7.6, 110.78, -7.5, 110.88] },
  { name: 'Magelang', bbox: [-7.52, 110.18, -7.42, 110.28] },
  { name: 'Madiun', bbox: [-7.68, 111.48, -7.58, 111.58] },
  { name: 'Kediri', bbox: [-7.85, 112.0, -7.75, 112.1] },
  { name: 'Probolinggo', bbox: [-7.78, 113.18, -7.68, 113.28] },
  { name: 'Pasuruan', bbox: [-7.68, 112.88, -7.58, 112.98] },
]

function cityTier(lat: number, lon: number): 0 | 1 | 2 {
  for (const c of TIER1_CITIES) if (inBbox(lat, lon, c.bbox)) return 1
  for (const c of TIER2_CITIES) if (inBbox(lat, lon, c.bbox)) return 2
  return 0
}

function tierMultiplier(tier: 0 | 1 | 2): number {
  return tier === 1 ? 2.0 : tier === 2 ? 1.4 : 1.0
}

function splitVehicles(aadt: number, tier: 0 | 1 | 2): { light: number; medium: number; heavy: number; moto: number } {
  if (tier === 1) {
    // Jakarta/Surabaya/etc: 60% motorcycles
    return {
      light: Math.round(aadt * 0.30),
      medium: Math.round(aadt * 0.05),
      heavy: Math.round(aadt * 0.05),
      moto: Math.round(aadt * 0.60),
    }
  }
  if (tier === 2) {
    return {
      light: Math.round(aadt * 0.40),
      medium: Math.round(aadt * 0.06),
      heavy: Math.round(aadt * 0.04),
      moto: Math.round(aadt * 0.50),
    }
  }
  return {
    light: Math.round(aadt * 0.50),
    medium: Math.round(aadt * 0.08),
    heavy: Math.round(aadt * 0.07),
    moto: Math.round(aadt * 0.35),
  }
}

const FILES = {
  toll: { relativePath: 'id/roads-toll.geojson', sha256: '053d0c7badcf4bae057470602e4f2b58c7b6d2202210bb41b6ae0f3e6293541d' },
  regional: { relativePath: 'id/roads-regional-lhrt.geojson', sha256: '16e90003889757c4e9c6e4e11982161010081527a1b465f512eb227e5ca96ca3' },
  national: { relativePath: 'id/roads-national.geojson', sha256: '14b87d21c1dd186b43fa0c41358e453f954263bd49d75614cffe830ca5c204b6' },
} as const

export interface IndonesiaRoadSource {
  toll: ReturnType<typeof buildRoadLineVertexGrid>
  regional: ReturnType<typeof buildRoadLineVertexGrid>
  national: ReturnType<typeof buildRoadLineVertexGrid>
  sourceRows: number
  sourceLines: number
  invalidGeometrySkipped: number
}

export function loadIndonesiaRoadSource(options: RoadLoaderArguments): IndonesiaRoadSource {
  const toll = loadPinnedRoadLines(options, [FILES.toll])
  const regional = loadPinnedRoadLines(options, [FILES.regional])
  const national = loadPinnedRoadLines(options, [FILES.national])
  return {
    toll: buildRoadLineVertexGrid(toll.lines),
    regional: buildRoadLineVertexGrid(regional.lines),
    national: buildRoadLineVertexGrid(national.lines),
    sourceRows: toll.sourceRows + regional.sourceRows + national.sourceRows,
    sourceLines: toll.lines.length + regional.lines.length + national.lines.length,
    invalidGeometrySkipped: toll.invalidGeometrySkipped + regional.invalidGeometrySkipped + national.invalidGeometrySkipped,
  }
}

const text = (line: PinnedRoadLine, key: string): string => String(line.properties[key] ?? '').trim()

export function matchIndonesiaRoad(row: RoadRow, source: IndonesiaRoadSource) {
  if (row.roadClass > 2) return null
  const tier = cityTier(row.midLat, row.midLon)
  const multiplier = tierMultiplier(tier)
  let total = 0
  let kind: 'toll' | 'lhrt' | 'regional' | 'national'
  if (nearestRoadLine(row.midLat, row.midLon, source.toll, 300)) {
    total = 80_000 * multiplier
    kind = 'toll'
  } else {
    const regional = nearestRoadLine(row.midLat, row.midLon, source.regional, 200)
    if (regional) {
      const rawLhrt = Number(regional.properties.LHRT ?? 0)
      const lhrt = Number.isFinite(rawLhrt) ? Math.min(Math.max(rawLhrt, 0), 200_000) : 0
      if (lhrt > 0) { total = lhrt; kind = 'lhrt' }
      else {
        const status = (text(regional, 'STATUS') || text(regional, 'ROAD_STATUS')).toLowerCase()
        total = (status.includes('kota') ? 12_000 : status.includes('provinsi') ? 8_000 : 5_000) * multiplier
        kind = 'regional'
      }
    } else if (nearestRoadLine(row.midLat, row.midLon, source.national, 400)) {
      total = 30_000 * multiplier
      kind = 'national'
    } else return null
  }
  const traffic = splitVehicles(total, tier)
  return Object.values(traffic).some(value => value > 0) ? { kind, ...traffic } : null
}

export const INDONESIA_ROAD_BBOX = ID_SCAN_BBOX
export const INDONESIA_ROAD_COVERAGE: ReadonlySet<number> = new Set([0, 1, 2])
