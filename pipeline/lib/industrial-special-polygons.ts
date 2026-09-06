/** Ordered Colombian concession containment, preserving source holes and tier precedence. */

import { inBbox, pointInRing } from './spatial.js'
import type { PreparedBbox } from './prepared-grid.js'
import type { SpecialFeature } from './industrial-special-source.js'
import { text } from './industrial-special-policy.js'

type Ring = readonly (readonly [number, number])[]
export interface IndustrialConcession { rings: readonly Ring[]; bbox: PreparedBbox; nace4: number }
function mineralNace(mineral: string): number {
  const value = mineral.toUpperCase()
  if (/CARBÓN|CARBON|ANTRACITA|HULLA|LIGNITO|TURBA/.test(value)) return 500
  if (/ORO|COBRE|HIERRO|PLATA|ZINC|PLOMO|NIQUEL|PLATIN|MOLIBDENO|MANGANESO|COBALTO|ESTAÑO|TUNGSTENO|MERCURIO|EMERALDA|ESMERALDA/.test(value)) return 700
  if (/ARENA|GRAVA|ARCILLA|CALIZA|MARMOL|GRANITO|YESO|FELDESPATO|CAOLIN|RECEBO|ROCA|CANTERA|BENTONITA|DOLOMITA|ANHIDRITA|FOSFORICA/.test(value)) return 800
  return 700 // Original ANM policy for other active mineral titles.
}

export function colombianConcessions(features: readonly SpecialFeature[], mining: boolean, countryBox: PreparedBbox) {
  const polygons: IndustrialConcession[] = []
  const counts = { raw: features.length, unlocated: 0, inactive: 0, outside: 0, admitted: 0, parts: 0, holes: 0 }
  for (const [index, feature] of features.entries()) {
    const geometry = feature.geometry, properties = feature.properties ?? {}
    if (!geometry || !['Polygon', 'MultiPolygon'].includes(geometry.type)) { counts.unlocated++; continue }
    if (mining ? !/Explotaci|Construcci/i.test(text(properties.ETAPA)) : !text(properties.ESTAD_AREA).toUpperCase().includes('PRODUC')) {
      counts.inactive++; continue
    }
    const parts = geometry.type === 'Polygon' ? [geometry.coordinates] : geometry.coordinates
    if (!Array.isArray(parts) || !parts.length) throw new Error(`invalid concession polygon ${index}`)
    let admitted = 0
    for (const part of parts) {
      if (!Array.isArray(part) || !part.length) throw new Error(`invalid concession rings ${index}`)
      const bbox: [number, number, number, number] = [90, 180, -90, -180]
      for (const [ringIndex, ring] of part.entries()) {
        if (!Array.isArray(ring) || ring.length < 4) throw new Error(`invalid concession ring ${index}`)
        for (const point of ring) {
          if (!Array.isArray(point) || point.length < 2 || typeof point[0] !== 'number' || typeof point[1] !== 'number' ||
              !Number.isFinite(point[0]) || !Number.isFinite(point[1]) || Math.abs(point[0]) > 180 || Math.abs(point[1]) > 90) {
            throw new Error(`invalid concession coordinate ${index}`)
          }
          if (ringIndex === 0) {
            bbox[0] = Math.min(bbox[0], point[1]); bbox[1] = Math.min(bbox[1], point[0])
            bbox[2] = Math.max(bbox[2], point[1]); bbox[3] = Math.max(bbox[3], point[0])
          }
        }
      }
      if (bbox[2] < countryBox[0] || bbox[0] > countryBox[2] || bbox[3] < countryBox[1] || bbox[1] > countryBox[3]) continue
      polygons.push({ rings: part as Ring[], bbox, nace4: mining ? mineralNace(text(properties.MINERALES)) : 600 })
      counts.parts++; counts.holes += part.length - 1; admitted++
    }
    if (admitted) counts.admitted++; else counts.outside++
  }
  if (!polygons.length) throw new Error('empty admitted Colombian concession source; refusing a partial reset')
  return { polygons, counts }
}

export function concessionClassifier(polygons: readonly IndustrialConcession[]) {
  // Original half-degree bbox index, retaining source order within each cell.
  const grid = new Map<string, IndustrialConcession[]>()
  for (const polygon of polygons) {
    for (let y = Math.floor(polygon.bbox[0] * 2); y <= Math.floor(polygon.bbox[2] * 2); y++) {
      for (let x = Math.floor(polygon.bbox[1] * 2); x <= Math.floor(polygon.bbox[3] * 2); x++) {
        const key = `${y}_${x}`, values = grid.get(key) ?? []
        values.push(polygon); grid.set(key, values)
      }
    }
  }
  return (lat: number, lon: number): number | null => {
    for (const polygon of grid.get(`${Math.floor(lat * 2)}_${Math.floor(lon * 2)}`) ?? []) {
      if (inBbox(lat, lon, polygon.bbox) && pointInRing(lon, lat, polygon.rings[0]) &&
          !polygon.rings.slice(1).some(hole => pointInRing(lon, lat, hole))) return polygon.nace4
    }
    return null
  }
}
