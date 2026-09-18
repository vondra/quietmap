/** Rebuild one source parent from finalized children and replace its Arrow file durably. */

import { closeSync, fsyncSync, openSync, renameSync, writeFileSync } from 'node:fs'
import { dirname } from 'node:path'
import { isDeepStrictEqual } from 'node:util'
import { tableToIPC, type Table } from 'apache-arrow'
import { transportPieceKey, type SourceParentGeometry } from './transport-topology.js'

/** Finalized rows of each source piece in file order. The first child speaks for the restored
 *  parent, so every sibling must carry the same value in each named attribute column. */
export function finalizedChildrenBySourcePiece(
  table: Table, attributes: readonly string[], family: 'railway' | 'road', square: string,
): Map<string, number[]> {
  const ids = table.getChild('osm_id'), indices = table.getChild('segment_idx')
  if (!ids || !indices) throw new Error(`${family} parent restoration requires source identity columns`)
  const vectors = attributes.map(name => table.getChild(name)!)
  // Flat arrays: a per-row Vector.get searches the batch list, which dominates a million-row square.
  const wayIds = ids.toArray() as BigInt64Array, segments = indices.toArray() as Int16Array
  const children = new Map<string, number[]>()
  for (let row = 0; row < table.numRows; row++) {
    const key = transportPieceKey(String(wayIds[row]), segments[row])
    const siblings = children.get(key)
    if (!siblings) { children.set(key, [row]); continue }
    vectors.forEach((vector, column) => {
      const first = vector.get(siblings[0]), value = vector.get(row)
      if (first !== value && !isDeepStrictEqual(first, value)) {
        throw new Error(`${family} children disagree on ${attributes[column]} for ${key} in ${square}`)
      }
    })
    siblings.push(row)
  }
  return children
}

/** Children must chain from the source start to the source end; finalization drops only subpixel children. */
export function sourceParentCoveredByChildren(
  table: Table, children: readonly number[], parent: SourceParentGeometry,
  family: 'railway' | 'road', piece: string,
): SourceParentGeometry {
  const { start, end } = parent
  const [startX, startY, endX, endY] = ['start_gx', 'start_gy', 'end_gx', 'end_gy'].map(name => table.getChild(name)!)
  const links = new Map<string, string>()
  const collapsed = new Set<string>()
  for (const child of children) {
    const from = `${startX.get(child)},${startY.get(child)}`
    const to = `${endX.get(child)},${endY.get(child)}`
    // Distinct metric cuts can quantize into the same z30 cell.
    if (from === to) { collapsed.add(from); continue }
    if (links.has(from)) throw new Error(`overlapping ${family} children for ${piece}`)
    links.set(from, to)
  }
  let endpoint: string | undefined = start.join(',')
  const edgeCount = links.size
  for (let visit = 0; visit < edgeCount; visit++) {
    collapsed.delete(endpoint!)
    const next: string | undefined = links.get(endpoint!)
    links.delete(endpoint!)
    endpoint = next
  }
  collapsed.delete(endpoint!)
  if (endpoint !== end.join(',') || links.size !== 0 || collapsed.size !== 0) {
    throw new Error(`${family} children do not cover source parent ${piece}`)
  }
  return parent
}

export function replaceArrowFileDurably(path: string, table: Table): void {
  const temporary = `${path}.tmp`
  const fd = openSync(temporary, 'w')
  try {
    writeFileSync(fd, Buffer.from(tableToIPC(table, 'file')))
    fsyncSync(fd)
  } finally { closeSync(fd) }
  renameSync(temporary, path)
  const directory = openSync(dirname(path), 'r')
  try { fsyncSync(directory) } finally { closeSync(directory) }
}
