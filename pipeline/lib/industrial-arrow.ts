/** Global one-site industrial classification through the canonical atomic Arrow writer. */

import { readFileSync, statSync, type BigIntStats } from 'node:fs'
import { resolve } from 'node:path'
import { DataType, makeVector, Table, tableFromIPC } from 'apache-arrow'
import { listPreparedSquares } from './prepared-grid.js'
import { withArrowWrite, shouldOverwrite } from './provenance.js'
import { PROVENANCE_RANK, SOURCES_BY_ID } from './sources.js'
import { buildOneHundredthDegreePointGrid, pointGridCandidates } from './spatial.js'
import { candidateEdgeM, contestBeats, overlapPairs, readPolygons, OVERLAP_MIN_AREA_M2,
  type MatchFacility, type MatchPolygon, type OverlapWinner } from './facility-match.js'

const SEARCH_RADIUS_M = 2000 // Actual dev1 registry centroid search horizon.
export interface IndustrialOwnership {
  facilityCountries: readonly string[]
  countryAt(table: Table): (row: number, polygon: MatchPolygon) => string | null
}
interface Winner { country: string; facility: MatchFacility; square: string; row: number; edge: number; polygon: MatchPolygon; existingSourceId: number }
const keyOf = (winner: Winner) => `${winner.square}:${winner.row}`

function stamps(table: Table, name: string, bits: number, optional = false): number[] {
  const column = table.getChild(name)
  if (!column && optional) return new Array<number>(table.numRows).fill(0)
  if (!column || !DataType.isInt(column.type) || column.type.bitWidth !== bits ||
      column.type.isSigned || column.nullCount !== 0) throw new Error(`industrial Arrow '${name}' must be non-null Uint${bits}`)
  return Array.from(column) as number[]
}

function sameIdentity(a: BigIntStats, b: BigIntStats): boolean {
  return a.dev === b.dev && a.ino === b.ino && a.size === b.size && a.mtimeNs === b.mtimeNs && a.ctimeNs === b.ctimeNs
}

export async function enrichIndustrialFacilities(
  preparedDirectory: string,
  facilities: readonly MatchFacility[],
  resetSourceIds: readonly number[],
  ownership?: IndustrialOwnership,
) {
  if (ownership && ownership.facilityCountries.length !== facilities.length) throw new Error('industrial facility ownership is not aligned')
  const reset = new Set(resetSourceIds)
  if (!reset.size || [...reset].some(id => SOURCES_BY_ID.get(id)?.layer !== 'industrial')) {
    throw new Error('industrial pass requires admitted industrial source identities')
  }
  for (const f of facilities) {
    if (!reset.has(f.id) || !Number.isInteger(f.nace4) || f.nace4 <= 0 || f.nace4 > 9999) {
      throw new Error('industrial facility has no admitted source or valid NACE')
    }
  }
  const squares = listPreparedSquares(preparedDirectory, [-90, -180, 90, 180], 'industrial.arrow')
  if (!squares.length) throw new Error(`${preparedDirectory}: no industrial Arrow scope`)
  const identities = new Map<string, BigIntStats>()
  const grid = buildOneHundredthDegreePointGrid(facilities.map((facility, index) =>
    ({ latitude: facility.lat, longitude: facility.lon, index })))
  const best = new Map<number, Winner>()
  const incumbents = new Map<string, OverlapWinner & { country: string }>()
  const previousOwned = new Map<string, OverlapWinner & { country: string }>()
  const result = { squares: squares.length, rows: 0, facilities: facilities.length, winners: 0,
    stamped: 0, suppressed: 0, reset: 0, squaresUpdated: 0 }

  // One read-only sweep proves every input before publication and reduces each
  // facility across all receiver owners. Only winners, not whole world tables, stay resident.
  for (const square of squares) {
    const path = resolve(preparedDirectory, square, 'industrial.arrow')
    const before = statSync(path, { bigint: true })
    const table = tableFromIPC(readFileSync(path))
    const polygons = readPolygons(table)
    const countryAt = ownership?.countryAt(table) ?? (() => '')
    const sourceIds = stamps(table, 'source_id', 16)
    const nace = stamps(table, 'nace_4digit', 16, true)
    stamps(table, 'suppressed', 8, true)
    if (!sameIdentity(before, statSync(path, { bigint: true }))) throw new Error(`${path}: changed during selection`)
    identities.set(square, before)
    result.rows += polygons.length
    for (const [row, polygon] of polygons.entries()) {
      const country = countryAt(row, polygon)
      if (country === null || polygon.sourceType === 10) continue
      const source = SOURCES_BY_ID.get(sourceIds[row])
      if (source?.layer === 'industrial' && nace[row] > 0 &&
          polygon.areaM2 >= OVERLAP_MIN_AREA_M2) {
        const key = `${square}:${row}`
        // The stored classification has authority but no retained registry-edge measurement.
        const target = reset.has(sourceIds[row]) ? previousOwned : incumbents
        target.set(key, { ...polygon, key, country, id: source.id,
          rank: PROVENANCE_RANK[source.provenance], year: source.year ?? 0 })
      }
      for (const { index } of pointGridCandidates(polygon.lat, polygon.lon, SEARCH_RADIUS_M, grid)) {
        if (ownership && ownership.facilityCountries[index] !== country) continue
        const facility = facilities[index]
        const edge = candidateEdgeM(facility, polygon, SEARCH_RADIUS_M)
        if (edge === null) continue
        const previous = best.get(index)
        if (!previous || edge < previous.edge) best.set(index, { country, facility, square, row, edge, polygon, existingSourceId: sourceIds[row] })
      }
    }
  }
  result.winners = best.size
  const contested = new Map<string, Winner>()
  // Original source observation order is the stable final tie breaker.
  for (const [, winner] of [...best].sort((a, b) => a[0] - b[0])) {
    const current = contested.get(keyOf(winner))
    if (!current || contestBeats({ ...winner.facility, edge: winner.edge },
      { ...current.facility, edge: current.edge })) contested.set(keyOf(winner), winner)
  }
  // Only published classifications participate: rejected global candidates cannot
  // lend their authority to an incumbent. Its actual source owns that decision.
  const applicable = new Map<string, Winner>()
  for (const winner of contested.values()) {
    const old = winner.existingSourceId
    if (shouldOverwrite(reset.has(old) ? 0 : old, winner.facility.id)) applicable.set(keyOf(winner), winner)
  }
  const overlap: Array<OverlapWinner & { country: string }> = [...applicable.values()].map(winner => ({
    ...winner.polygon, ...winner.facility, lat: winner.polygon.lat, lon: winner.polygon.lon,
    key: keyOf(winner), country: winner.country, edge: winner.edge,
  }))
  overlap.push(...[...incumbents].filter(([key]) => !applicable.has(key)).map(([, row]) => row))
  const pairsByCountry = (rows: Array<OverlapWinner & { country: string }>) => {
    const groups = new Map<string, OverlapWinner[]>()
    for (const row of rows) {
      const group = groups.get(row.country) ?? []
      group.push(row); groups.set(row.country, group)
    }
    return [...groups.values()].flatMap(group => overlapPairs(group))
  }
  const affected = new Set<string>()
  // Only a verified former duplicate may wake after this pass's winner retires;
  // unrelated suppressed incumbents retain their prepared state.
  for (const [winner, loser] of pairsByCountry([...previousOwned.values(), ...incumbents.values()])) {
    if (previousOwned.has(winner) || previousOwned.has(loser)) { affected.add(winner); affected.add(loser) }
  }
  // Resetting a registry stamp does not remove its original OSM emitter. Keep
  // that verified duplicate in the final election with its actual baseline authority.
  const baseline = SOURCES_BY_ID.get(0)!
  for (const [key, row] of previousOwned) {
    if (affected.has(key) && !applicable.has(key)) overlap.push({ ...row, id: baseline.id,
      rank: PROVENANCE_RANK[baseline.provenance], year: baseline.year ?? 0 })
  }
  const suppression = new Map<string, number>()
  for (const row of overlap) if (affected.has(row.key)) suppression.set(row.key, 0)
  for (const [winner, loser] of pairsByCountry(overlap)) {
    if (!applicable.has(winner) && !applicable.has(loser) && !affected.has(winner) && !affected.has(loser)) continue
    suppression.set(winner, 0)
    suppression.set(loser, 1)
  }
  result.suppressed = [...suppression.values()].filter(value => value !== 0).length

  for (const square of squares) {
    const path = resolve(preparedDirectory, square, 'industrial.arrow')
    await withArrowWrite(path, table => {
      if (!sameIdentity(identities.get(square)!, statSync(path, { bigint: true }))) {
        throw new Error(`${path}: changed after global selection; rerun against stable prepared input`)
      }
      const countryAt = ownership?.countryAt(table) ?? (() => '')
      const polygons = readPolygons(table)
      const oldSource = stamps(table, 'source_id', 16)
      const oldNace = stamps(table, 'nace_4digit', 16, true)
      const oldSuppressed = stamps(table, 'suppressed', 8, true)
      const source = Uint16Array.from(oldSource)
      const nace = Uint16Array.from(oldNace)
      const suppressedColumn = Uint8Array.from(oldSuppressed)
      for (let row = 0; row < table.numRows; row++) {
        if (countryAt(row, polygons[row]) === null || (ownership && polygons[row].sourceType === 10)) continue
        if (reset.has(source[row])) {
          source[row] = 0; nace[row] = 0; suppressedColumn[row] = 0
          result.reset++
        }
        const key = `${square}:${row}`
        const winner = applicable.get(key)
        if (winner) {
          source[row] = winner.facility.id; nace[row] = winner.facility.nace4
          suppressedColumn[row] = 0
          result.stamped++
        }
        const electedSuppression = suppression.get(key)
        if (electedSuppression !== undefined) suppressedColumn[row] = electedSuppression
      }
      if (source.every((value, row) => value === oldSource[row] && nace[row] === oldNace[row] &&
          suppressedColumn[row] === oldSuppressed[row])) return table
      const columns: Record<string, import('apache-arrow').Vector> = {}
      for (const field of table.schema.fields) columns[field.name] = table.getChild(field.name)!
      columns.source_id = makeVector(source)
      columns.nace_4digit = makeVector(nace)
      columns.suppressed = makeVector(suppressedColumn)
      result.squaresUpdated++
      return new Table(columns)
    })
  }
  return result
}
