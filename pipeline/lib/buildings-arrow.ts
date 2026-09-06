/** Atomic national building refinements on the current buildings_v3/z30 contract. */

import { DataType, makeVector, Table, type Vector } from 'apache-arrow'
import { withArrowWrite } from './provenance.js'
import { gridToLonLat } from './prepared-grid.js'
import { SOURCES_BY_ID, shouldOverwrite } from './sources.js'

export interface BuildingRow {
  lat: number
  lon: number
  floors: number
  buildingType: number
  existingSourceId: number
}

export interface BuildingPatch {
  floors?: number
  buildingType?: number
  sourceId: number
}

function integerColumn(table: Table, name: string, bits: number, signed = false): Vector {
  const column = table.getChild(name)
  if (!column || !DataType.isInt(column.type) || column.type.bitWidth !== bits ||
      column.type.isSigned !== signed || column.nullCount !== 0) {
    throw new Error(`buildings Arrow '${name}' must be non-null ${signed ? 'Int' : 'Uint'}${bits}`)
  }
  return column
}

export async function writeBuildingEnrichment(
  path: string,
  match: (row: BuildingRow) => BuildingPatch | null,
) {
  const result = { rows: 0, matched: 0, floorsAdded: 0, typesChanged: 0, typeDowngradesBlocked: 0, updated: false }
  await withArrowWrite(path, table => {
    if (table.schema.metadata.get('buildings_contract') !== 'buildings_v3' ||
        table.schema.metadata.get('grid') !== 'z30') {
      throw new Error(`${path}: expected buildings_v3/z30 contract`)
    }
    const gx = integerColumn(table, 'centroid_gx', 32, true)
    const gy = integerColumn(table, 'centroid_gy', 32, true)
    const originalFloors = integerColumn(table, 'floors', 8)
    const originalTypes = integerColumn(table, 'building_type', 8)
    const originalSource = integerColumn(table, 'source_id', 16)
    result.rows = table.numRows
    const floors = Uint8Array.from(originalFloors)
    const types = Uint8Array.from(originalTypes)
    const sources = Uint16Array.from(originalSource)
    for (let i = 0; i < table.numRows; i++) {
      if (types[i] > 13) throw new Error(`${path}: invalid building_type at row ${i}`)
      const row: BuildingRow = {
        ...gridToLonLat(gx.get(i) as number, gy.get(i) as number),
        floors: floors[i], buildingType: types[i], existingSourceId: sources[i],
      }
      const patch = match(row)
      if (!patch) continue
      if ((patch.floors !== undefined && (!Number.isInteger(patch.floors) || patch.floors < 0 || patch.floors > 255)) ||
          (patch.buildingType !== undefined && (!Number.isInteger(patch.buildingType) || patch.buildingType < 0 || patch.buildingType > 13)) ||
          SOURCES_BY_ID.get(patch.sourceId)?.layer !== 'buildings') {
        throw new Error(`${path}: invalid building refinement at row ${i}: ${JSON.stringify(patch)}`)
      }
      if (!shouldOverwrite(sources[i], patch.sourceId)) continue
      const nextFloors = patch.floors ?? floors[i]
      // National coarse use codes cannot erase explicit OSM/POI classes 10–13.
      const blocked = types[i] >= 10 && patch.buildingType !== undefined && patch.buildingType < 10
      const nextType = blocked ? types[i] : patch.buildingType ?? types[i]
      if (blocked) result.typeDowngradesBlocked++
      if (nextFloors === floors[i] && nextType === types[i]) continue
      if (nextFloors !== floors[i]) result.floorsAdded++
      if (nextType !== types[i]) result.typesChanged++
      floors[i] = nextFloors
      types[i] = nextType
      sources[i] = patch.sourceId
      result.matched++
    }
    if (!result.matched) return table
    result.updated = true
    const columns: Record<string, Vector> = {}
    for (const field of table.schema.fields) columns[field.name] = table.getChild(field.name)!
    columns.floors = makeVector(floors)
    columns.building_type = makeVector(types)
    columns.source_id = makeVector(sources)
    return new Table(columns)
  })
  return result
}
