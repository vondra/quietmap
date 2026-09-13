/** Reuse a rail search only for identical directed coordinates and shape geometry. */

import type { RailStationPairCount } from './rail-graph.js'

export class RailPairSearches {
  private readonly searches = new Map<string, RailStationPairCount>()
  private readonly shapeIds = new Map<string, number>()
  private readonly shapeReferences = new WeakMap<Array<[number, number]>, number>()

  add(pair: RailStationPairCount): void {
    let shapeId = 0
    if (pair.shapePolyline) {
      const shape = pair.shapePolyline
      let id = this.shapeReferences.get(shape)
      if (id === undefined) {
        const geometry = JSON.stringify(shape)
        id = this.shapeIds.get(geometry)
        if (id === undefined) {
          id = this.shapeIds.size + 1
          this.shapeIds.set(geometry, id)
        }
        this.shapeReferences.set(shape, id)
      }
      shapeId = id
    }
    const key = `${pair.fromLat},${pair.fromLon}|${pair.toLat},${pair.toLon}|${shapeId}`
    const existing = this.searches.get(key)
    if (existing) {
      existing.pax += pair.pax
      existing.frt += pair.frt
    } else {
      this.searches.set(key, { ...pair })
    }
  }

  get size(): number { return this.searches.size }
  values(): MapIterator<RailStationPairCount> { return this.searches.values() }
}
