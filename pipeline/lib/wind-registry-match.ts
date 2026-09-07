/**
 * Nearest-record lookup and the industrial.arrow write shared by every
 * wind-turbine registry enricher — `enrich-industrial-{ca,de,dk,es,no,se}.ts`
 * and `enrich-global-windturbines.ts` — which used to carry a copy each.
 *
 * Three rules every copy had to get right on its own, and did not:
 *   - the block of cells scanned around a row follows from the SEARCH RADIUS
 *     and the row's latitude, never a fixed 3x3 ring;
 *   - a registry value may only FILL a spec the row is missing, never
 *     overwrite one the row already carries;
 *   - the patched hex is published through the shared locked, atomic Arrow
 *     write path, never a bare `writeFileSync` over the live file.
 */

import { makeTable, makeVector, Table } from 'apache-arrow'
import { withArrowWrite } from './provenance.js'
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

/** What a registry offers for one OSM turbine row. A zero means the registry
 *  publishes no such spec (NVE has no hub heights), never "set it to zero". */
export interface RegistryTurbineSpecs { hubHeightM: number; ratedPowerKw: number }

export interface WriteTurbineSpecsResult {
  /** Rows in this hex that are OSM wind turbines (`source_type` 10). */
  turbineRows: number
  /** Turbine rows for which the registry returned a record. */
  matched: number
  /** Turbine rows whose hub height or rated power this pass filled in. */
  filled: number
}

/**
 * Fill `hub_height` / `rated_power_kw` on one hex's OSM wind-turbine rows from
 * a registry, through the same locked + tmp + rename path the road and rail
 * writers use (`withArrowWrite`, which also re-imposes the input's schema
 * metadata, field flags and batch boundaries). The seven registry enrichers
 * each rebuilt the table and `writeFileSync`-d it over the live file instead:
 * unlocked and truncating in place, so a painter reading that hex could see a
 * torn file, and any schema metadata added later would be silently dropped.
 *
 * A hex where nothing was filled returns the input table, which leaves its
 * bytes untouched — the `hexFilled === 0` guard each caller used to spell out.
 *
 * Both columns carry no Arrow null bitmap, so "unknown" is whatever the Rust
 * reader refuses: it keeps a spec only when `value > 0.0` — `pos_f32` in
 * engine/tile-painter/src/source_loader_industrial.rs:103-105 for the painter,
 * the same test inline in engine/source-reader/src/query.rs:449-465 for the
 * popup — which makes 0, every negative and NaN alike mean "no value", and the
 * engine then falls back to its 80 m / 2000 kW defaults. An absent column
 * therefore materializes as all-zero and an existing value, whatever its sign,
 * rides through the copy loop verbatim: there is nothing to normalize.
 */
export async function writeTurbineSpecs(
  arrowPath: string,
  specsAt: (lat: number, lon: number) => RegistryTurbineSpecs | null,
): Promise<WriteTurbineSpecsResult> {
  let turbineRows = 0
  let matched = 0
  let filled = 0

  await withArrowWrite(arrowPath, (table: Table): Table => {
    const n = table.numRows
    if (n === 0) return table

    const sourceTypes = table.getChild('source_type')
    const lats = table.getChild('centroid_lat')
    const lons = table.getChild('centroid_lon')
    if (!sourceTypes || !lats || !lons) return table // malformed hex — never touch

    const existingHub = table.getChild('hub_height')
    const existingPower = table.getChild('rated_power_kw')
    // Verbatim copy — see the sentinel paragraph above: a non-positive or NaN
    // value is already unknown to both readers, so scrubbing it to 0 would only
    // rewrite bytes without changing a single level the visitor reads.
    const hubHeightM = new Float32Array(n)
    const ratedPowerKw = new Float32Array(n)
    for (let i = 0; i < n; i++) {
      hubHeightM[i] = (existingHub?.get(i) as number) ?? 0
      ratedPowerKw[i] = (existingPower?.get(i) as number) ?? 0
    }

    for (let i = 0; i < n; i++) {
      if (((sourceTypes.get(i) as number) ?? 0) !== 10) continue
      turbineRows++
      const lat = (lats.get(i) as number) ?? 0
      const lon = (lons.get(i) as number) ?? 0
      if (lat === 0 || lon === 0) continue // Null Island row — no registry to match
      const specs = specsAt(lat, lon)
      if (!specs) continue
      matched++
      if (fillMissingTurbineSpecs(hubHeightM, ratedPowerKw, i, specs.hubHeightM, specs.ratedPowerKw)) filled++
    }
    if (filled === 0) return table // no change → withArrowWrite leaves bytes untouched

    // eslint-disable-next-line @typescript-eslint/no-explicit-any -- makeTable types its
    // record as TypedArray-only, but the untouched columns ride through as Vectors
    // (see roads-arrow.ts); fine at runtime.
    const columns: Record<string, any> = {}
    for (const field of table.schema.fields) {
      if (field.name === 'hub_height' || field.name === 'rated_power_kw') continue
      columns[field.name] = table.getChild(field.name)!
    }
    columns['hub_height'] = makeVector(hubHeightM)
    columns['rated_power_kw'] = makeVector(ratedPowerKw)
    return makeTable(columns)
  })

  return { turbineRows, matched, filled }
}

/**
 * `writeTurbineSpecs` for a national register: every one of them parses its
 * download into records carrying both specs under these two field names, so the
 * nearest record within `radiusM` is the whole rule. NVE (`enrich-industrial-no`)
 * publishes power per PARK rather than per turbine and keeps the callback form.
 */
export function writeTurbineSpecsFromGrid<
  T extends RegistryRecordPosition & { hub_height_m: number; rated_power_kw: number },
>(arrowPath: string, grid: Map<string, T[]>, radiusM: number): Promise<WriteTurbineSpecsResult> {
  return writeTurbineSpecs(arrowPath, (lat, lon) => {
    const best = findNearestRegistryRecord(grid, lat, lon, radiusM)
    return best ? { hubHeightM: best.hub_height_m, ratedPowerKw: best.rated_power_kw } : null
  })
}
