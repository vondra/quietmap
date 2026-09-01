/**
 * Reproduce EBA Lärm-Monitoring synthetic receivers on their VzG line
 * validation v2 rail lane): the 2023 snapshot shipped TOWN CENTROIDS
 * (coord_uncertainty_m 3000, levels trend-only). Each station name carries its
 * VzG number ("Telgte, Strecke 2200: …"); German OSM tracks carry that number
 * in `ref`, so the mast can be snapped to the nearest matching-ref rail
 * segment in our own arrows. This establishes a repeatable 7.5 m model
 * receiver, not the unpublished mast position; along-track uncertainty stays
 * explicit and may be kilometres even on a nominally homogeneous corridor.
 * The approved coordinates are now frozen in eba-stations.json and emitted
 * atomically by snapshot-eba.ts. This script is audit-only: it recomputes the
 * snap against a prepared dataset and refuses drift; it never rewrites an
 * approved snapshot behind the adapter's back.
 *
 * Run: npx tsx pipeline/validation/geolocate-eba-masts.ts
 */
import { readFileSync, existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { tableFromIPC } from 'apache-arrow'
import { latLngToCell, gridDisk } from 'h3-js'
import { H3R4_DIR } from '../lib/data-year.js'

const SNAP_PATH = resolve(import.meta.dirname, '../../benchmarks/validation/snapshots/eba-laermmonitoring.2023.json')

const snap = JSON.parse(readFileSync(SNAP_PATH, 'utf8'))
const M_PER_DEG_LAT = 110_540

let snapped = 0
for (const st of snap.stations) {
  const strecke = /Strecke (\d+)/.exec(st.name || '')?.[1]
  if (!strecke) { console.log(`${st.station_id}: no Strecke number in name — kept centroid`); continue }
  // Idempotent: re-runs search from the original town centroid, not a
  // previously-snapped trackside point.
  const origin = st.town_centroid ?? { lat: st.lat, lng: st.lng }
  let best: { lat: number; lng: number; dist: number; dlat: number; dlng: number } | null = null
  for (const hex of gridDisk(latLngToCell(origin.lat, origin.lng, 4), 1)) {
    const f = resolve(H3R4_DIR, hex, 'railways.arrow')
    if (!existsSync(f)) continue
    const t = tableFromIPC(readFileSync(f))
    const ref = t.getChild('ref')
    const sla = t.getChild('start_lat')!, slo = t.getChild('start_lon')!
    const ela = t.getChild('end_lat')!, elo = t.getChild('end_lon')!
    const cosLat = Math.cos((origin.lat * Math.PI) / 180)
    for (let i = 0; i < t.numRows; i++) {
      const r = ref?.get(i) as string | null
      // VzG refs match exactly or as a ";"-separated member (interlaced tags).
      if (!r || !(r === strecke || r.split(/[;,]\s*/).includes(strecke))) continue
      const mlat = (sla.get(i)! + ela.get(i)!) / 2
      const mlng = (slo.get(i)! + elo.get(i)!) / 2
      const d = Math.hypot((mlat - origin.lat) * M_PER_DEG_LAT, (mlng - origin.lng) * 111_320 * cosLat)
      if (!best || d < best.dist)
        best = { lat: mlat, lng: mlng, dist: d, dlat: ela.get(i)! - sla.get(i)!, dlng: elo.get(i)! - slo.get(i)! }
    }
  }
  if (best) {
    // The mast stands at the standardized 7.5 m pass-by distance — a receiver
    // ON the track reads ~8-10 dB hotter than the mast for pure geometry.
    // Offset perpendicular to the segment, toward the town side (the mast is
    // between line and settlement more often than not; free-field either way).
    const cosLat = Math.cos((origin.lat * Math.PI) / 180)
    const tx = best.dlng * 111_320 * cosLat
    const ty = best.dlat * M_PER_DEG_LAT
    const len = Math.hypot(tx, ty) || 1
    let nx = -ty / len, ny = tx / len // unit normal in metres
    const toTownX = (origin.lng - best.lng) * 111_320 * cosLat
    const toTownY = (origin.lat - best.lat) * M_PER_DEG_LAT
    if (nx * toTownX + ny * toTownY < 0) { nx = -nx; ny = -ny }
    const MAST_OFFSET_M = 7.5
    const candidateLat = Number((best.lat + (ny * MAST_OFFSET_M) / M_PER_DEG_LAT).toFixed(6))
    const candidateLng = Number((best.lng + (nx * MAST_OFFSET_M) / (111_320 * cosLat)).toFixed(6))
    const driftM = Math.hypot(
      (candidateLat - st.lat) * M_PER_DEG_LAT,
      (candidateLng - st.lng) * 111_320 * cosLat,
    )
    if (driftM > 1) throw new Error(`${st.station_id}: prepared rail geometry moved frozen receiver by ${driftM.toFixed(1)} m — review and update eba-stations.json explicitly`)
    console.log(`${st.station_id}: frozen Strecke ${strecke} receiver reproduced within ${driftM.toFixed(1)} m`)
    snapped++
  } else {
    console.log(`${st.station_id}: Strecke ${strecke} NOT FOUND within ring-1 — kept centroid`)
  }
}

if (snapped !== snap.stations.length) throw new Error(`only ${snapped}/${snap.stations.length} frozen receivers reproduced`)
console.log(`\nverified ${snapped}/${snap.stations.length} frozen EBA receivers; no files written`)
