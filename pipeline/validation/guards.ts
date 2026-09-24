/**
 * Criteria v2 guards: each sub-check's predicate read from the criteria text and judged on the popup
 * at the guard point (pass / fail / not_scored with its quantity); a regression compares two runs.
 */
import type { Criteria, CriteriaGuard } from './criteria.ts'
import type { Manifest } from './manifest.ts'
import type { Contributor, StationModel } from './popup.ts'
import type { GuardPointRow, StationRow } from './report.ts'
import { metresBetween } from './popup.ts'

export type SubcheckResult = { guard: string; subcheck: string; state: 'pass' | 'fail' | 'not_scored'; quantity_db: number | null; detail: string }

/** The criteria's layer words → popup layers. */
const LAYERS: Record<string, string> = { industrial: 'industrial', road: 'road', rail: 'railway', railway: 'railway', aircraft: 'aircraft' }
/** A catalogue station within this distance of a guard coordinate is that guard's station. */
const GUARD_STATION_MATCH_M = 10

const round2 = (value: number) => Math.round(value * 100) / 100
const loudest = (model: StationModel, layer: string): Contributor | null =>
  model.contributors.filter(contributor => contributor.source_type === layer).sort((a, b) => b.received_lden - a.received_lden)[0] ?? null
function shareOfLayer(model: StationModel, layer: string, osmId: number): number | null {
  const total = model.layers[layer]?.lden
  const own = model.contributors.find(contributor => contributor.source_type === layer && contributor.osm_id === osmId)
  return total == null || !own ? null : 10 ** (own.received_lden / 10) / 10 ** (total / 10)
}

export function evaluateGuardSubchecks(
  guard: CriteriaGuard, points: GuardPointRow[], stations: StationRow[], manifest: Manifest | null, criteria: Criteria,
): SubcheckResult[] {
  const tau = criteria.constants.tau_db.value
  const siblingId = guard.subchecks.map(check => /osm_id == (\d+)/.exec(check.predicate)?.[1]).find(Boolean)
  const result = (subcheck: string, state: SubcheckResult['state'], quantity: number | null, detail: string): SubcheckResult =>
    ({ guard: guard.id, subcheck, state, quantity_db: quantity == null ? null : round2(quantity), detail })
  // "OSM id: catalogue to record": the catalogue's guard row at the same point names the source.
  const catalogueRow = guard.lat == null ? null
    : stations.find(row => row.guard_osm && metresBetween(row.lat, row.lng, guard.lat!, guard.lng!) <= GUARD_STATION_MATCH_M) ?? null
  const catalogueId = catalogueRow?.guard_osm ? Number(catalogueRow.guard_osm.split('/').at(-1)) : null
  const pointModel = (label: string | null) => {
    const point = points.find(entry => entry.guard === guard.id && (label == null || entry.label === label))
    return point?.model ?? null
  }
  return guard.subchecks.flatMap(check => {
    const text = check.predicate
    let match: RegExpExecArray | null
    if (/survey record/i.test(text)) return [result(check.id, 'not_scored', null, 'needs a survey record')]
    if (/OSM id: catalogue to record/i.test(text)) {
      const layer = LAYERS[/(\w+) contributors?/.exec(text)?.[1] ?? '']
      const model = pointModel(null)
      if (catalogueId == null || !layer || !model) return [result(check.id, 'not_scored', null, 'the catalogue has not recorded the OSM id')]
      const ranked = model.contributors.filter(contributor => contributor.source_type === layer).sort((a, b) => b.received_lden - a.received_lden)
      const topN = Number(/among the top (\d+)/.exec(text)?.[1] ?? 1)
      const found = ranked.slice(0, topN).some(contributor => contributor.osm_id === catalogueId)
      return [result(check.id, found ? 'pass' : 'fail', null, `${catalogueRow!.guard_osm} in the top ${topN}: ${found}; top: ${ranked.slice(0, topN).map(entry => `${entry.osm_id} ${entry.name}`).join(', ')}`)]
    }
    if ((match = /^at (\d+) m the top (\w+) contributor has subtype (\w+)/.exec(text))) {
      const model = pointModel(`${match[1]} m`)
      if (!model) return [result(check.id, 'not_scored', null, `no popup at ${match[1]} m`)]
      const top = loudest(model, LAYERS[match[2]])
      return [result(check.id, top?.subtype === match[3] ? 'pass' : 'fail', null, `top ${match[2]}: ${top ? `${top.osm_id} ${top.name} (${top.subtype})` : 'none'}`)]
    }
    const model = pointModel(null)
    if ((match = /^top (\w+) contributor osm_id == (\d+)/.exec(text))) {
      if (!model) return [result(check.id, 'not_scored', null, 'no popup at the guard point')]
      const top = loudest(model, LAYERS[match[1]])
      return [result(check.id, top?.osm_id === Number(match[2]) ? 'pass' : 'fail', null, `top ${match[1]}: ${top ? `${top.osm_id} ${top.name}` : 'none'}`)]
    }
    const shareRule = /share of that contributor in (\w+)-layer energy >= ([\d.]+)/.exec(text)
    const namedShareRule = /contributor osm_id == (\d+) .*?>= ([\d.]+) of (\w+)-layer energy/.exec(text)
    if (shareRule || namedShareRule) {
      const [osmId, threshold, layerWord] = shareRule ? [siblingId, Number(shareRule[2]), shareRule[1]]
        : [namedShareRule![1], Number(namedShareRule![2]), namedShareRule![3]]
      if (!model || !osmId) return [result(check.id, 'not_scored', null, 'no popup or no OSM id')]
      const share = shareOfLayer(model, LAYERS[layerWord], Number(osmId))
      if (share == null) return [result(check.id, 'fail', null, `${osmId} is not among the listed ${layerWord} contributors`)]
      const quantity = 10 * Math.log10(share / threshold)
      return [result(check.id, quantity >= 0 ? 'pass' : 'fail', quantity, `share ${(100 * share).toFixed(1)} % (threshold ${100 * threshold} %)`)]
    }
    if ((match = /AADT \/ (\d+)\)\| <= tau if .*applied, <= (\d+) dB if modelled/.exec(text))) {
      if (!model || !siblingId) return [result(check.id, 'not_scored', null, 'no popup or no OSM id')]
      const road = model.contributors.find(contributor => contributor.osm_id === Number(siblingId) && contributor.aadt_total != null)
      if (!road?.aadt_total) return [result(check.id, 'fail', null, `road ${siblingId} is not listed or carries no traffic`)]
      const quantity = Math.abs(10 * Math.log10(road.aadt_total / Number(match[1])))
      const applied = /DfT|Department for Transport/i.test(road.dataset_name ?? '')
      const limit = applied ? tau : Number(match[2])
      return [result(check.id, quantity <= limit ? 'pass' : 'fail', quantity, `${road.aadt_total}/day vs ${match[1]} (${applied ? 'count applied' : 'modelled'}, limit ${limit} dB)`)]
    }
    if (/aircraft layer <= road layer/.test(text)) {
      if (!model) return [result(check.id, 'not_scored', null, 'no popup at the guard point')]
      const quantity = (model.layers.aircraft?.lden ?? -Infinity) - (model.layers.road?.lden ?? -Infinity)
      return [result(check.id, quantity <= 0 ? 'pass' : 'fail', Number.isFinite(quantity) ? quantity : null, 'aircraft minus road Lden')]
    }
    if (/every layer Lden <= measured Lden/.test(text) && guard.lat != null && guard.lng != null) {
      const station = stations.find(row => metresBetween(row.lat, row.lng, guard.lat!, guard.lng!) <= GUARD_STATION_MATCH_M)
      const entry = station && manifest?.stations[station.key]?.metrics.lden
      if (!station?.model || !entry) return [result(check.id, 'not_scored', null, 'no scored catalogue station at the guard')]
      const topLayer = Math.max(...Object.values(station.model.layers).map(layer => layer.lden ?? -Infinity))
      const excess = Math.max(...Object.entries(entry.O_by_variant).map(([variant, O]) =>
        topLayer - (O! + criteria.constants.upper_bound_k.value * entry.u_i_by_variant[variant as keyof typeof entry.u_i_by_variant]!)))
      return [result(check.id, excess <= 0 ? 'pass' : 'fail', excess, `loudest layer ${round2(topLayer)} dB against measured + ${criteria.constants.upper_bound_k.value} u_i`)]
    }
    if ((match = /model AADT of the counted road \/ count\)\| <= tau when the count dataset is the row provenance, else <= (\d+) dB/.exec(text))) {
      return (guard.members ?? []).map(member => {
        const station = stations.find(row => row.key === member)
        const comparison = station?.comparisons.find(entry => entry.kind === 'traffic' && entry.period === 'total')
        if (!station?.model || comparison?.delta_db == null) return result(`${check.id} ${member}`, 'not_scored', null, 'no traffic comparison')
        const measuredRow = ['city-measured', 'national-measured', 'continental-measured', 'global-measured'].includes(station.model.dominant_road?.provenance_tier ?? '')
        const limit = measuredRow ? tau : Number(match![1])
        const quantity = Math.abs(comparison.delta_db)
        return result(`${check.id} ${member}`, quantity <= limit ? 'pass' : 'fail', quantity, `${comparison.model}/day vs ${comparison.measured} (limit ${limit} dB)`)
      })
    }
    return [result(check.id, 'not_scored', null, 'predicate not machine-readable')]
  })
}

/** Criteria C4: pass → fail, or fail → fail with the quantity worse by more than τ. */
export function subcheckRegressions(before: SubcheckResult[], after: SubcheckResult[], criteria: Criteria): string[] {
  const tau = criteria.constants.tau_db.value
  return after.flatMap(checkB => {
    const checkA = before.find(entry => entry.guard === checkB.guard && entry.subcheck === checkB.subcheck)
    if (!checkA) return []
    const worse = checkA.quantity_db != null && checkB.quantity_db != null && Math.abs(checkB.quantity_db) - Math.abs(checkA.quantity_db) > tau
    return (checkA.state === 'pass' && checkB.state === 'fail') || (checkA.state === 'fail' && checkB.state === 'fail' && worse)
      ? [`${checkB.guard}/${checkB.subcheck}: ${checkA.state} ${checkA.quantity_db ?? ''} → ${checkB.state} ${checkB.quantity_db ?? ''}`] : []
  })
}
