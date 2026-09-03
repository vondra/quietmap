/**
 * Per-hex country resolution for enrichment WRITERS (service-tree national
 * vehicle mix / trip rates).
 *
 * Each cell's admin.bin assigns one country per ~22 km res-4 hex
 * (build-h3-admin.ts: centroid PIP + interior max-share sampling, CGAZ) — fine
 * for diagnostics,
 * forbidden for traffic-data writes at borders (a Czech road in the Hlučínsko
 * salient got Polish AADT that way; see country-polygon.ts). The split here:
 * INTERIOR hexes (own ISO defined AND every resolved k=1 neighbour agrees)
 * take the cheap per-cell answer; everything else resolves each query point
 * through AdminAt, the ONE global CGAZ point resolver (plan M2 §1).
 *
 * An own-undefined hex is NEVER interior: with no ISO of its own the k=1
 * ring can never nominate a country — the Koh Phangan sea-cluster trap, where
 * the whole ring is UNKNOWN and the nearest TH hex is at k=2, classified
 * "interior" and stamped WORLD over every segment without one PIP test.
 *
 * Construction fails loud when the admin records are missing/empty
 * (requireAdminIso) — a silent WORLD fallback across a whole extract is the
 * exact failure the 2026-07 /gg review caught.
 */

import { cellToBoundary, gridDisk } from 'h3-js'
import { requireAdminIso } from './admin-iso.js'
import { adminAt } from './admin-at.js'
import { allCountryPolygonBboxes } from './country-polygon.js'

export interface HexCountryResolver {
  /** admin-record ISO2 of the hex itself (undefined when the cell carries no
   *  country — ocean/unassigned hexes). */
  hexIso(hexId: string): string | undefined
  /** True when the hex is NOT interior: own ISO undefined, or any resolved
   *  k=1 neighbour carries a different country. */
  isBorderHex(hexId: string): boolean
  /** Country at a point: the hex ISO for interior hexes; otherwise AdminAt's
   *  exact/coastal CGAZ answer, falling back to the hex ISO (when defined) for
   *  points no polygon claims — never a first-candidate coin-flip. */
  isoAt(hexId: string, lat: number, lon: number): string | undefined
}

export function createHexCountryResolver(h3r4Dir: string): HexCountryResolver {
  const adminIso = requireAdminIso(h3r4Dir)

  // Interior status + own ISO, cached per hex (each hex is queried once per
  // segment otherwise).
  const interiorInfo = new Map<string, { interior: boolean; own: string | undefined }>()
  function infoFor(hexId: string): { interior: boolean; own: string | undefined } {
    let info = interiorInfo.get(hexId)
    if (info === undefined) {
      const own = adminIso.get(hexId)
      let interior = own !== undefined // own-undefined ⇒ NEVER interior (see header)
      if (interior) {
        for (const neighbour of gridDisk(hexId, 1)) {
          const iso = adminIso.get(neighbour)
          // Neighbours with no country record (open ocean) don't disagree —
          // coastal hexes stay interior and keep the zero-PIP fast path.
          if (iso !== undefined && iso !== own) {
            interior = false
            break
          }
        }
      }
      if (interior && enclosesForeignPart(hexId, own!)) interior = false
      // Sliver probe (/gg M2 Codex): k=1 centroid agreement is NOT
      // containment — a border can clip a hex corner (AT hex with a DE
      // sliver). Interior only when every resolved interior sample agrees
      // with `own`; unresolved (sea) samples don't disagree.
      if (interior && !allLandSamplesAgree(hexId, own!)) interior = false
      info = { interior, own }
      interiorInfo.set(hexId, info)
    }
    return info
  }

  // ~19 interior samples (centroid + vertices + edge midpoints + half-radius
  // ring), pentagon-safe (res-4 has 12 pentagons — do not assume 6 vertices).
  function hexSamples(hexId: string): [number, number][] {
    const boundary = cellToBoundary(hexId)
    const pts: [number, number][] = []
    const push = (a: [number, number], b: [number, number], t: number) =>
      pts.push([a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])])
    const cLat = boundary.reduce((s, p) => s + p[0], 0) / boundary.length
    const cLon = boundary.reduce((s, p) => s + p[1], 0) / boundary.length
    const c: [number, number] = [cLat, cLon]
    pts.push(c)
    for (let i = 0; i < boundary.length; i++) {
      const v = boundary[i]
      const nx = boundary[(i + 1) % boundary.length]
      pts.push(v)
      push(v, nx, 0.5)
      push(v, c, 0.5)
    }
    return pts
  }

  function allLandSamplesAgree(hexId: string, own: string): boolean {
    for (const [lat, lon] of hexSamples(hexId)) {
      const iso = adminAt(lat, lon).iso2
      if (iso !== undefined && iso !== own) return false
    }
    return true
  }

  // Enclave trigger (Vatican / San Marino / Monaco class): a hex that fully
  // encloses a FOREIGN CGAZ part must never be interior — the k=1 agreement
  // test alone classifies Rome's Vatican hex "interior IT" and stamps IT over
  // VA without ever calling AdminAt. Only a border TRIGGER: per-point
  // resolution is AdminAt's, no candidate list lives here anymore.
  const PAD_DEG = 0.3 // ≈ one k-ring of slop around the ~22 km hex
  function enclosesForeignPart(hexId: string, own: string): boolean {
    const boundary = cellToBoundary(hexId) // [[lat, lon], …]
    let s = Infinity, w = Infinity, n = -Infinity, e = -Infinity
    for (const [lat, lon] of boundary) {
      if (lat < s) s = lat
      if (lat > n) n = lat
      if (lon < w) w = lon
      if (lon > e) e = lon
    }
    for (const [iso, parts] of Object.entries(allCountryPolygonBboxes())) {
      if (iso === own) continue
      for (const [ps, pw, pn, pe] of parts) {
        if (ps >= s - PAD_DEG && pn <= n + PAD_DEG && pw >= w - PAD_DEG && pe <= e + PAD_DEG) return true
      }
    }
    return false
  }

  return {
    hexIso: (hexId) => adminIso.get(hexId),
    isBorderHex: (hexId) => !infoFor(hexId).interior,
    isoAt(hexId, lat, lon) {
      const info = infoFor(hexId)
      if (info.interior) return info.own
      // Border or own-undefined hex: AdminAt's exact CGAZ PIP with the
      // uniquely-attributable 2 km coastal fallback built in. Points no
      // polygon claims keep the hex's own ISO when defined (piers/reclaimed
      // land on an otherwise-interior hex's fringe), else undefined → WORLD.
      return adminAt(lat, lon).iso2 ?? info.own
    },
  }
}
