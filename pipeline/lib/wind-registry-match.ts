/**
 * Nearest-record lookup shared by every wind-turbine registry enricher —
 * `enrich-industrial-{ca,de,dk,es,no,se}.ts` and
 * `enrich-global-windturbines.ts` — which used to carry a copy each.
 *
 * Two rules every copy had to get right on its own, and did not:
 *   - the block of cells scanned around a row follows from the SEARCH RADIUS
 *     and the row's latitude, never a fixed 3x3 ring;
 *   - a registry value may only FILL a spec the row is missing, never
 *     overwrite one the row already carries.
 */

import { haversineM, M_PER_DEG_LAT, M_PER_DEG_LON_EQ } from './spatial.js'

/** Registry index cell: ~1.1 km of latitude, and between 353 m (Alaska's
 *  71.5°) and 1,113 m of longitude depending on how far north the row sits. */
const REGISTRY_GRID_CELL_DEG = 0.01

export interface RegistryRecordPosition { lat: number; lon: number }

export function buildRegistryGrid<T extends RegistryRecordPosition>(records: readonly T[]): Map<string, T[]> {
  const grid = new Map<string, T[]>()
  for (const record of records) {
    const key = `${Math.floor(record.lat / REGISTRY_GRID_CELL_DEG)},${Math.floor(record.lon / REGISTRY_GRID_CELL_DEG)}`
    const cell = grid.get(key)
    if (cell) cell.push(record)
    else grid.set(key, [record])
  }
  return grid
}

/**
 * The nearest registry record within `radiusM`, or null.
 *
 * The scanned block is derived from the radius because a cell is not square in
 * metres: 0.01° of longitude is 380 m at 70° N, so the fixed 3x3 ring every
 * copy of this loop used could not reach 500 m. (70, 10.009) and (70, 10.021)
 * are 456 m apart but two cells apart in x — unreachable before, found now.
 */
export function findNearestRegistryRecord<T extends RegistryRecordPosition>(
  grid: Map<string, T[]>, lat: number, lon: number, radiusM: number,
): T | null {
  const cellY = Math.floor(lat / REGISTRY_GRID_CELL_DEG)
  const cellX = Math.floor(lon / REGISTRY_GRID_CELL_DEG)
  const blockY = Math.ceil(radiusM / (M_PER_DEG_LAT * REGISTRY_GRID_CELL_DEG))
  const cosLat = Math.max(Math.cos(lat * Math.PI / 180), 0.01) // poles: clamp, never divide by 0
  const blockX = Math.ceil(radiusM / (M_PER_DEG_LON_EQ * cosLat * REGISTRY_GRID_CELL_DEG))

  let nearest: T | null = null
  let nearestDistM = radiusM
  for (let dy = -blockY; dy <= blockY; dy++) {
    for (let dx = -blockX; dx <= blockX; dx++) {
      const cell = grid.get(`${cellY + dy},${cellX + dx}`)
      if (!cell) continue
      for (const record of cell) {
        const d = haversineM(lat, lon, record.lat, record.lon)
        if (d < nearestDistM) { nearestDistM = d; nearest = record }
      }
    }
  }
  return nearest
}

/**
 * Writes a matched record's hub height and rated power into the row's columns,
 * but ONLY where the registry carries a positive value and the row carries
 * none (0 and NaN both mean "unknown" in these Arrow columns — they have no
 * null bitmap). Returns whether anything actually changed, so a caller only
 * rewrites a hex it really altered.
 *
 * The per-country copies assigned both fields unconditionally, so a registry
 * zero erased a measured OSM value: Swedish node 10695841749 lost its 45 kW to
 * a Vindbrukskollen record with no power, and the engine then fell back to its
 * 2000 kW default — +7 dB on that turbine.
 */
export function fillMissingTurbineSpecs(
  hubHeightM: Float32Array, ratedPowerKw: Float32Array, row: number,
  registryHubHeightM: number, registryRatedPowerKw: number,
): boolean {
  let filled = false
  if (registryHubHeightM > 0 && !(hubHeightM[row] > 0)) { hubHeightM[row] = registryHubHeightM; filled = true }
  if (registryRatedPowerKw > 0 && !(ratedPowerKw[row] > 0)) { ratedPowerKw[row] = registryRatedPowerKw; filled = true }
  return filled
}
