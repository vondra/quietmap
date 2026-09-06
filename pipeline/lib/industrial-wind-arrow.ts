/** Fill native turbine parameters in original global-then-national order under one Arrow lock. */

import { DataType, Float32, Table, vectorFromArray } from 'apache-arrow'
import { withArrowWrite } from './provenance.js'
import { gridToLonLat, GRID_CONTRACT } from './prepared-grid.js'
import { buildOneHundredthDegreePointGrid, pointGridCandidates, flatDist, haversineM, inBbox } from './spatial.js'
import type { WindObservation, WindRegister } from './industrial-wind-source.js'

export function windParameterMatcher(registers: readonly WindRegister[]) {
  const indexed = registers.map(register => ({ ...register,
    grid: buildOneHundredthDegreePointGrid(register.observations.filter(p => register.country !== 'US' || p.power > 0)),
  }))
  const us = registers.find(r => r.country === 'US')
  if (!us || new Set(registers.map(r => r.country)).size !== registers.length) throw new Error('wind family must include one USWTDB register')
  const globalGrid = buildOneHundredthDegreePointGrid(us.observations)
  function nearest(lat: number, lon: number, radius: number, grid: ReturnType<typeof buildOneHundredthDegreePointGrid<WindObservation>>, distance: typeof flatDist) {
    let best: WindObservation | null = null, bestDistance = radius
    for (const observation of pointGridCandidates(lat, lon, radius, grid)) {
      const metres = distance(lat, lon, observation.latitude, observation.longitude)
      if (metres < bestDistance) { best = observation; bestDistance = metres }
    }
    return best
  }
  return (lat: number, lon: number, initialHub: number | null, initialPower: number | null) => {
    let hub = initialHub, power = initialPower
    // The actual chain's US-only global pass fills each absent field separately.
    const global = nearest(lat, lon, 200, globalGrid, flatDist)
    if (global) {
      if (!(hub != null && hub > 0) && global.hub > 0) hub = global.hub
      if (!(power != null && power > 0) && global.power > 0) power = global.power
    }
    for (const register of indexed) {
      if (!inBbox(lat, lon, register.bbox) || !lat || !lon ||
          (register.country === 'NO' ? (power ?? 0) > 0 : (hub ?? 0) > 0 && (power ?? 0) > 0)) continue
      const best = nearest(lat, lon, register.radiusM, register.grid, haversineM)
      if (!best) continue
      if (register.country !== 'NO' && best.hub > 0) hub = best.hub
      if (best.power > 0) power = best.power
    }
    return { hub: hub === null ? null : Math.fround(hub), power: power === null ? null : Math.fround(power) }
  }
}

export async function enrichWindSquare(path: string, match: ReturnType<typeof windParameterMatcher>) {
  const result = { rows: 0, turbines: 0, changed: 0, updated: false }
  await withArrowWrite(path, table => {
    if (table.schema.metadata.get('grid') !== GRID_CONTRACT) throw new Error('wind Arrow must use the native z30 grid contract')
    const type = table.getChild('source_type'), gx = table.getChild('centroid_gx'), gy = table.getChild('centroid_gy')
    if (!type || !DataType.isInt(type.type) || type.type.isSigned || type.type.bitWidth !== 8 || type.nullCount) throw new Error('wind source_type must be non-null Uint8')
    for (const column of [gx, gy]) if (!column || !DataType.isInt(column.type) || !column.type.isSigned || column.type.bitWidth !== 32 || column.nullCount) throw new Error('wind centroid must be native non-null Int32')
    const hubs = table.getChild('hub_height'), powers = table.getChild('rated_power_kw')
    for (const column of [hubs, powers]) if (!column || !DataType.isFloat(column.type) || column.type.precision !== 1) throw new Error('wind measurements must be nullable Float32')
    const hub = Array.from(hubs!) as Array<number | null>, power = Array.from(powers!) as Array<number | null>
    result.rows = table.numRows
    for (let row = 0; row < table.numRows; row++) {
      if (type.get(row) !== 10) continue
      result.turbines++
      if ([hub[row], power[row]].some(value => value !== null && !Number.isFinite(value))) throw new Error(`invalid native wind measurement at row ${row}`)
      const { lat, lon } = gridToLonLat(gx!.get(row) as number, gy!.get(row) as number)
      const next = match(lat, lon, hub[row], power[row])
      if (next.hub === hub[row] && next.power === power[row]) continue
      hub[row] = next.hub; power[row] = next.power; result.changed++
    }
    if (!result.changed) return table
    const columns: Record<string, import('apache-arrow').Vector> = {}
    for (const field of table.schema.fields) columns[field.name] = table.getChild(field.name)!
    columns.hub_height = vectorFromArray(hub, new Float32())
    columns.rated_power_kw = vectorFromArray(power, new Float32())
    result.updated = true
    return new Table(columns)
  })
  return result
}
