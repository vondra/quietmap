/** Reuse a rail search only for identical directed coordinates and shape geometry. */

import type { RailStationPairCount } from './rail-graph.js'

export class RailShapeIndex {
  private readonly ids = new Map<string, number>()
  private readonly references = new WeakMap<Array<[number, number]>, number>()

  idFor(shape: Array<[number, number]>): number {
    let id = this.references.get(shape)
    if (id === undefined) {
      const geometry = JSON.stringify(shape)
      id = this.ids.get(geometry)
      if (id === undefined) {
        id = this.ids.size + 1
        this.ids.set(geometry, id)
      }
      this.references.set(shape, id)
    }
    return id
  }

  entries(): MapIterator<[string, number]> { return this.ids.entries() }
}

export class RailPairSearches {
  private readonly searches = new Map<string, RailStationPairCount>()
  private readonly shapes = new RailShapeIndex()

  add(pair: RailStationPairCount): void {
    const shapeId = pair.shapePolyline ? this.shapes.idFor(pair.shapePolyline) : 0
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
