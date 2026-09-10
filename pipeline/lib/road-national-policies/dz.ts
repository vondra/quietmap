/** DZ OSM-class national traffic policy ported from the dev1 published policy. */

import { inBbox, pointToPolylineDist } from '../spatial.js'
import type { NationalRoadPolicy, PolicyRoadTraffic } from './types.js'

const DZ_SCAN_BBOX: [number, number, number, number] = [18.9, -8.7, 37.1, 12.0]

// ── City tiers (AADT multiplier vs rural baseline) ──
interface City { name: string; lat: number; lon: number; tier: 1 | 2; half: number }

const CITIES: City[] = [
  // Tier 1 ×2.0 — box wide enough to cover the Algiers conurbation (Bab Ezzouar,
  // El Harrach, Dar El Beïda, Hussein Dey).
  { name: 'Grand Alger', lat: 36.75, lon: 3.06, tier: 1, half: 0.25 },
  // Tier 2 ×1.4 — wilaya capitals + ports + Sahara oil/gas hubs.
  { name: 'Oran',            lat: 35.70, lon: -0.63, tier: 2, half: 0.14 },
  { name: 'Constantine',     lat: 36.36, lon: 6.61,  tier: 2, half: 0.12 },
  { name: 'Annaba',          lat: 36.90, lon: 7.77,  tier: 2, half: 0.10 },
  { name: 'Blida',           lat: 36.47, lon: 2.83,  tier: 2, half: 0.10 },
  { name: 'Batna',           lat: 35.55, lon: 6.17,  tier: 2, half: 0.10 },
  { name: 'Djelfa',          lat: 34.67, lon: 3.26,  tier: 2, half: 0.10 },
  { name: 'Setif',           lat: 36.19, lon: 5.41,  tier: 2, half: 0.10 },
  { name: 'Sidi Bel Abbes',  lat: 35.19, lon: -0.63, tier: 2, half: 0.10 },
  { name: 'Biskra',          lat: 34.85, lon: 5.73,  tier: 2, half: 0.10 },
  { name: 'Tebessa',         lat: 35.40, lon: 8.12,  tier: 2, half: 0.10 },
  { name: 'Tlemcen',         lat: 34.88, lon: -1.32, tier: 2, half: 0.10 },
  { name: 'Bejaia',          lat: 36.75, lon: 5.08,  tier: 2, half: 0.10 },
  { name: 'Tiaret',          lat: 35.37, lon: 1.32,  tier: 2, half: 0.10 },
  { name: 'Bechar',          lat: 31.62, lon: -2.22, tier: 2, half: 0.10 },
  { name: 'Skikda',          lat: 36.88, lon: 6.91,  tier: 2, half: 0.10 },
  { name: 'Chlef',           lat: 36.17, lon: 1.33,  tier: 2, half: 0.10 },
  { name: 'Mostaganem',      lat: 35.93, lon: 0.09,  tier: 2, half: 0.10 },
  { name: 'Ouargla',         lat: 31.95, lon: 5.32,  tier: 2, half: 0.10 },
  { name: 'Ghardaia',        lat: 32.48, lon: 3.67,  tier: 2, half: 0.10 },
  { name: 'Laghouat',        lat: 33.80, lon: 2.88,  tier: 2, half: 0.10 },
  { name: 'Hassi Messaoud',  lat: 31.68, lon: 6.07,  tier: 2, half: 0.10 },
  { name: "Hassi R'Mel",     lat: 32.93, lon: 3.27,  tier: 2, half: 0.10 },
  { name: 'Adrar',           lat: 27.87, lon: -0.29, tier: 2, half: 0.10 },
  { name: 'Tamanrasset',     lat: 22.79, lon: 5.52,  tier: 2, half: 0.10 },
  { name: 'Tizi Ouzou',      lat: 36.72, lon: 4.05,  tier: 2, half: 0.10 },
  { name: 'El Oued',         lat: 33.37, lon: 6.86,  tier: 2, half: 0.10 },
  { name: 'Boumerdes',       lat: 36.77, lon: 3.48,  tier: 2, half: 0.10 },
]

function cityTier(lat: number, lon: number): 0 | 1 | 2 {
  for (const c of CITIES) {
    if (lat >= c.lat - c.half && lat <= c.lat + c.half &&
        lon >= c.lon - c.half && lon <= c.lon + c.half) return c.tier
  }
  return 0
}

// ── Corridor polylines [lon,lat] — matched by min distance to the whole line ──
// Hassi R'Mel gas hub → Ghardaïa → Ouargla → Hassi Messaoud oil capital → In
// Amenas: the RN3 + oilfield-logistics spine carrying tanker/heavy freight.
const OILGAS_CORRIDOR: [number, number][] = [
  [3.27, 32.93], [3.67, 32.48], [5.32, 31.95], [6.07, 31.68], [9.55, 28.05],
]
// Autoroute Est-Ouest (A1, ~1,216 km): Tunisia border ↔ Annaba ↔ Constantine ↔
// Sétif ↔ Algiers ↔ Blida ↔ Chlef ↔ Oran ↔ Tlemcen ↔ Morocco border. Coastal,
// car-dominated — lower heavy share than the interior RN network.
const AUTOROUTE_EW: [number, number][] = [
  [8.24, 36.78], [7.77, 36.90], [6.61, 36.36], [5.41, 36.19], [3.48, 36.77],
  [3.06, 36.75], [2.83, 36.47], [1.33, 36.17], [-0.63, 35.70], [-1.32, 34.88], [-1.75, 34.80],
]

const nearOilGas = (lat: number, lon: number) => pointToPolylineDist(lat, lon, OILGAS_CORRIDOR) <= 25000
const nearAutoroute = (lat: number, lon: number) => pointToPolylineDist(lat, lon, AUTOROUTE_EW) <= 12000

// ── Region → vehicle split (fractions sum to 1.0) ──
type Region = 'tier1' | 'tier2' | 'autoroute' | 'oilgas' | 'rural'
interface Split { light: number; medium: number; heavy: number; moto: number }
const SPLITS: Record<Region, Split> = {
  tier1:     { light: 0.66, medium: 0.12, heavy: 0.13, moto: 0.09 },
  tier2:     { light: 0.66, medium: 0.10, heavy: 0.16, moto: 0.08 },
  autoroute: { light: 0.74, medium: 0.05, heavy: 0.17, moto: 0.04 },
  oilgas:    { light: 0.38, medium: 0.05, heavy: 0.52, moto: 0.05 },
  rural:     { light: 0.58, medium: 0.07, heavy: 0.29, moto: 0.06 },
}

/** Region for the vehicle split: cities first, then the oil-gas freight corridor
 *  (deep south), then the coastal Autoroute Est-Ouest, else the rural RN network. */
function region(lat: number, lon: number, tier: 0 | 1 | 2): Region {
  if (tier === 1) return 'tier1'
  if (tier === 2) return 'tier2'
  if (nearOilGas(lat, lon)) return 'oilgas'
  if (nearAutoroute(lat, lon)) return 'autoroute'
  return 'rural'
}

// ── AADT defaults by OSM road_class (engine inputs.rs codes) ──
const BASE_AADT: Record<number, number> = {
  0: 35000, 10: 17500, // motorway + motorway_link
  1: 12000, 11: 6000,  // trunk + trunk_link
  2: 6000,  12: 3000,  // primary + primary_link
  3: 3000,             // secondary
  4: 1500,             // tertiary
  5: 700,              // residential
}
const DZ_COVERAGE: ReadonlySet<number> = new Set(Object.keys(BASE_AADT).map(Number))

function splitVehicles(aadt: number, s: Split) {
  return {
    light: Math.round(aadt * s.light),
    medium: Math.round(aadt * s.medium),
    heavy: Math.round(aadt * s.heavy),
    moto: Math.round(aadt * s.moto),
  }
}

function traffic(row: Parameters<NationalRoadPolicy['traffic']>[0]): PolicyRoadTraffic | null {
  const base = BASE_AADT[row.roadClass]
  if (base === undefined) return null
  const tier = cityTier(row.midLat, row.midLon)
    return splitVehicles(base * (tier === 1 ? 2 : tier === 2 ? 1.4 : 1), SPLITS[region(row.midLat, row.midLon, tier)])
}

export const policy: NationalRoadPolicy = {
  country: 'DZ',
  bbox: DZ_SCAN_BBOX,
  coverage: DZ_COVERAGE,
  traffic,
}
