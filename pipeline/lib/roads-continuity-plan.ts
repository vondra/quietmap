/** Redistribute measured four-class flow through one native owner graph. */

import { isMeasured } from './sources.js'
import { SOURCE_ID_ROAD_CONTINUITY_HEURISTIC } from './source-ids.generated.js'
import { DATASETS } from './enrichment-datasets.js'
import { classDefaultTotal, type PlanningRoad } from './road-planning-input.js'

const MY_SOURCE_ID = SOURCE_ID_ROAD_CONTINUITY_HEURISTIC
// Dev1: conflicting anchors above 3× veto the segment; otherwise the lower wins.
export const CONFLICT_RATIO = 3
export const FILLABLE = new Set(DATASETS.find(row => row.id === MY_SOURCE_ID)!.roadCoverage!)
interface Flow { light: number; medium: number; heavy: number; moto: number; total: number; anchor: number }
function scaleFlow(v: Flow, total: number, anchor: number): Flow {
  const factor = total > 0 && v.total > 0 ? total / v.total : 0
  return { light: v.light * factor, medium: v.medium * factor, heavy: v.heavy * factor, moto: v.moto * factor, total, anchor }
}

export function buildContinuityPlan(roads: PlanningRoad[]) {
  const segs = roads.map(road => ({ ...road, source: road.src,
    light: road.aadt[0], medium: road.aadt[1], heavy: road.aadt[2], moto: road.aadt[3],
    total: road.aadt.reduce((sum, value) => sum + value, 0) }))
  const endpoint = new Map<string, number[]>()
  for (const road of segs) for (const key of [road.a, road.b]) {
    const rows = endpoint.get(key)
    if (rows) rows.push(road.i)
    else endpoint.set(key, [road.i])
  }
  // Prior self output is never a new measurement or a measured side-road draw.
  const drawEstimate = (road: typeof segs[number]) => isMeasured(road.source) ? road.total : classDefaultTotal(road.cls)
  const fill = new Map<number, Flow>()
  const conflicted = new Set<number>()
  let conflicts = 0
  const record = (t: number, v: Flow): boolean => {
    if (conflicted.has(t)) return false
    const prev = fill.get(t)
    if (prev && prev.anchor !== v.anchor) {
      const ratio = Math.max(prev.total, v.total) / Math.max(1, Math.min(prev.total, v.total))
      if (ratio > CONFLICT_RATIO) {
        fill.delete(t)
        conflicted.add(t)
        conflicts++
        return false
      }
      if (v.total < prev.total) fill.set(t, v) // conservative: keep the lower
      return true
    }
    fill.set(t, v)
    return true
  }
  for (const A of segs) {
    if (!isMeasured(A.source) || A.total <= 0) continue
    const seen = new Set<number>([A.i])
    const queue: Array<{ seg: number; v: Flow }> = [
      { seg: A.i, v: { light: A.light, medium: A.medium, heavy: A.heavy, moto: A.moto, total: A.total, anchor: A.i } },
    ]
    let head = 0
    while (head < queue.length) {
      const { seg, v } = queue[head++]
      for (const ep of [segs[seg].a, segs[seg].b]) {
        const outs = (endpoint.get(ep) ?? []).filter((j) => j !== seg)
        if (outs.length === 0) continue
        const sameRef = segs[seg].ref ? outs.filter((o) => segs[o].ref === segs[seg].ref) : []
        const targets: Array<{ seg: number; total: number }> = []
        if (sameRef.length >= 1) {
          let drawn = 0
          for (const o of outs) if (!sameRef.includes(o)) drawn += drawEstimate(segs[o])
          const mainTotal = Math.max(0, v.total - drawn)
          for (const p of sameRef) targets.push({ seg: p, total: mainTotal / sameRef.length })
        } else {
          let totalDef = 0
          for (const o of outs) totalDef += classDefaultTotal(segs[o].cls)
          if (totalDef <= 0) continue
          for (const o of outs) targets.push({ seg: o, total: (v.total * classDefaultTotal(segs[o].cls)) / totalDef })
        }

        for (const { seg: t, total: tTotal } of targets) {
          if (seen.has(t)) continue
          const o = segs[t]
          if (!FILLABLE.has(o.cls) || (o.source !== 0 && o.source !== MY_SOURCE_ID)) continue
          if (tTotal <= classDefaultTotal(o.cls)) continue
          seen.add(t)
          const vt = scaleFlow(v, tTotal, v.anchor)
          if (record(t, vt)) queue.push({ seg: t, v: vt })
        }
      }
    }
  }

  return { fill, conflicts, anchors: segs.filter(road => isMeasured(road.source) && road.total > 0).length }
}
