/** Dev1's operator-published railway proxy tiers for nine African countries. */

import {
  SOURCE_ID_CD_NATIONAL_RAILWAY, SOURCE_ID_DZ_NATIONAL_RAILWAY,
  SOURCE_ID_EG_NATIONAL_RAILWAY, SOURCE_ID_ET_NATIONAL_RAILWAY,
  SOURCE_ID_KE_NATIONAL_RAILWAY, SOURCE_ID_MA_NATIONAL_RAILWAY,
  SOURCE_ID_NG_NATIONAL_RAILWAY, SOURCE_ID_SD_NATIONAL_RAILWAY,
  SOURCE_ID_TZ_NATIONAL_RAILWAY,
} from './source-ids.generated.js'
import { flatDist, pointToPolylineDist } from './spatial.js'
import {
  inCentreBox, inCoordinateBox, nearAnyPolyline, nearPolyline, trains,
  type RailwayProxySpec,
} from './railway-proxy-rules.js'
import type { RailwayRow } from './railways-arrow.js'

type Line = ReadonlyArray<readonly [number, number]>

const CD_CFMK: Line = [[13.45, -5.82], [13.92, -5.70], [14.44, -5.56], [15.10, -5.13], [15.31, -4.40]]
const CD_COPPERBELT: Line = [[27.48, -11.66], [26.73, -10.98], [26.07, -10.60], [25.40, -9.50], [24.99, -8.74]]
const CD_KOLWEZI: Line = [[26.07, -10.60], [25.47, -10.72]]
const CD_DORMANT: Line = [[24.99, -8.74], [23.60, -7.00], [22.42, -5.90], [20.58, -4.33]]
function classifyCd(row: RailwayRow) {
  if (row.railType !== 0 && row.railType !== 3) return null
  if (nearPolyline(row, CD_CFMK, 8_000)) return trains(2, 4)
  if (nearAnyPolyline(row, [CD_COPPERBELT, CD_KOLWEZI], 8_000)) return trains(1, 3)
  if (nearPolyline(row, CD_DORMANT, 8_000)) return trains(1, 1)
  if (row.usage === 2) return trains(0, 2)
  return trains(1, 1)
}

const DZ_NORTHERN: Line = [
  [-0.63, 35.70], [-0.10, 35.78], [0.56, 35.74], [1.01, 36.00], [1.33, 36.17],
  [2.22, 36.26], [2.83, 36.47], [3.06, 36.75], [3.55, 36.73], [3.90, 36.38],
  [4.76, 36.07], [5.41, 36.19], [5.69, 36.15], [6.61, 36.36], [7.10, 36.40],
  [7.43, 36.46], [7.77, 36.90],
]
const DZ_PHOSPHATE: Line = [[7.95, 34.70], [8.12, 35.40], [8.13, 35.95], [7.77, 36.90]]
function classifyDz(row: RailwayRow) {
  if (row.railType === 2) return trains(150, 0)
  if (row.railType === 1) {
    if (inCentreBox(row, 36.75, 3.06, 0.25)) return trains(80, 0)
    if (inCentreBox(row, 35.70, -0.63, 0.15)) return trains(60, 0)
    return trains(40, 0)
  }
  if (row.railType !== 0) return null
  if (row.usage === 2) return trains(0, 6)
  if (nearPolyline(row, DZ_PHOSPHATE, 12_000)) return trains(1, 18)
  if (nearPolyline(row, DZ_NORTHERN, 12_000)) return trains(15, 12)
  return row.usage === 1 ? trains(1, 3) : trains(4, 6)
}

const EG_CAIRO_ALEX: Line = [[31.247, 30.063], [31.18, 30.46], [30.97, 30.79], [30.47, 31.03], [29.90, 31.19]]
const EG_UPPER: Line = [[31.247, 30.063], [31.10, 29.07], [30.75, 28.10], [31.18, 27.18], [31.70, 26.56], [32.72, 26.16], [32.64, 25.70], [32.90, 24.09]]
const EG_SUEZ: readonly Line[] = [
  [[31.247, 30.063], [31.50, 30.59], [32.27, 30.60], [32.30, 31.26]],
  [[32.27, 30.60], [32.55, 29.97]],
]
function classifyEg(row: RailwayRow) {
  if (row.railType === 1) return trains(250, 0)
  if (row.railType === 2) return trains(400, 0)
  if (row.railType !== 0) return null
  if (row.usage === 2) return trains(0, 6)
  if (nearPolyline(row, EG_CAIRO_ALEX, 10_000)) return trains(100, 30)
  if (nearPolyline(row, EG_UPPER, 10_000)) return trains(40, 15)
  if (nearAnyPolyline(row, EG_SUEZ, 10_000)) return trains(20, 15)
  return row.usage === 1 ? trains(1, 4) : trains(10, 8)
}

const ET_EDR: Line = [
  [38.60, 8.92], [38.80, 8.86], [38.99, 8.74], [39.11, 8.59], [39.27, 8.54],
  [39.55, 8.62], [39.92, 8.90], [40.17, 8.99], [40.45, 9.10], [40.75, 9.24],
  [41.20, 9.45], [41.70, 9.55], [41.86, 9.59], [42.00, 9.95], [42.20, 10.45],
  [42.50, 11.00], [42.65, 11.15],
]
const ET_AKR: Line = [[40.17, 8.99], [40.05, 9.55], [40.00, 10.05], [39.95, 10.55], [39.85, 10.90], [39.74, 11.08], [39.63, 11.13], [39.62, 11.45], [39.60, 11.72]]
function classifyEt(row: RailwayRow) {
  if (row.railType === 1 || row.railType === 2) return trains(150, 0)
  if (row.railType === 4) return trains(2, 0)
  if (row.railType === 3) return trains(1, 1)
  if (row.railType !== 0) return null
  const name = row.name.toLowerCase()
  if (name.includes('weldiya') || name.includes('woldia')) return trains(1, 4)
  if (name.includes('djibouti') || row.name.includes('ጅቡቲ') || row.name.includes('جيبوتي')) return trains(4, 12)
  // The Awash tie belongs to the busier EDR, matching dev1's `<=` rule.
  const edrDistance = pointToPolylineDist(row.midLat, row.midLon, ET_EDR)
  const akrDistance = pointToPolylineDist(row.midLat, row.midLon, ET_AKR)
  return edrDistance <= akrDistance ? trains(4, 12) : trains(1, 4)
}

const KE_SGR1: Line = [[39.55, -4.03], [38.86, -3.50], [38.556, -3.396], [38.169, -2.690], [37.66, -2.30], [37.12, -1.90], [36.95, -1.45], [36.90, -1.34]]
const KE_SGR2: Line = [[36.90, -1.34], [36.70, -1.40], [36.55, -1.30], [36.42, -1.05], [36.35, -0.95]]
function classifyKe(row: RailwayRow) {
  if (row.railType !== 0) return null
  if (row.usage === 2) return trains(0, 4)
  if (nearPolyline(row, KE_SGR1, 12_000)) return trains(8, 20)
  if (nearPolyline(row, KE_SGR2, 12_000)) return trains(4, 8)
  if (flatDist(row.midLat, row.midLon, -1.286, 36.817) <= 30_000) return trains(20, 4)
  return trains(1, 4)
}

const MA_ATLANTIC: Line = [[-5.80, 35.77], [-6.04, 35.47], [-6.15, 35.18], [-6.58, 34.26], [-6.82, 34.02], [-7.38, 33.69], [-7.59, 33.57]]
const MA_EASTERN: Line = [[-6.58, 34.26], [-5.71, 34.22], [-5.55, 33.90], [-5.00, 34.04], [-4.01, 34.21], [-2.89, 34.41], [-1.91, 34.68]]
const MA_MARRAKECH: Line = [[-7.59, 33.57], [-7.59, 33.27], [-7.62, 33.00], [-7.95, 32.24], [-8.01, 31.63]]
const MA_PHOSPHATE: readonly Line[] = [
  [[-6.91, 32.88], [-7.50, 32.92], [-8.10, 33.00], [-8.63, 33.13]],
  [[-7.95, 32.24], [-8.53, 32.25], [-9.24, 32.30]],
]
function classifyMa(row: RailwayRow) {
  if (row.railType === 1 || row.railType === 2) {
    if (inCentreBox(row, 33.57, -7.59, 0.25)) return trains(300, 0)
    if (inCentreBox(row, 34.02, -6.82, 0.20)) return trains(250, 0)
    return trains(200, 0)
  }
  if (row.railType !== 0) return null
  if (row.usage === 2) return trains(0, 8)
  if (nearAnyPolyline(row, MA_PHOSPHATE, 10_000)) return trains(1, 60)
  if (nearPolyline(row, MA_EASTERN, 12_000)) return trains(40, 15)
  if (nearPolyline(row, MA_ATLANTIC, 12_000)) return trains(40, 8)
  if (nearPolyline(row, MA_MARRAKECH, 12_000)) return trains(30, 10)
  return row.usage === 1 ? trains(4, 3) : trains(8, 6)
}

const NG_LAGOS_IBADAN: Line = [[3.39, 6.47], [3.36, 6.70], [3.34, 7.15], [3.62, 7.30], [3.90, 7.40], [3.90, 7.45]]
const NG_ABUJA_KADUNA: Line = [[7.42, 9.00], [7.30, 9.40], [7.40, 10.00], [7.45, 10.52]]
const NG_ITAKPE_WARRI: Line = [[6.30, 7.60], [6.66, 7.55], [6.50, 7.00], [6.19, 6.25], [5.75, 5.52]]
function classifyNg(row: RailwayRow) {
  if (row.railType === 1 || row.railType === 2) {
    if (inCoordinateBox(row, [6.40, 3.28, 6.70, 3.55])) return trains(250, 0)
    return trains(60, 0)
  }
  if (row.usage === 2) return trains(0, 6)
  if (row.railType === 0) {
    if (inCoordinateBox(row, [6.43, 3.28, 6.72, 3.43])) return trains(250, 20)
    if (nearPolyline(row, NG_LAGOS_IBADAN, 6_000)) return trains(16, 20)
    if (nearPolyline(row, NG_ABUJA_KADUNA, 6_000)) return trains(8, 6)
    if (nearPolyline(row, NG_ITAKPE_WARRI, 8_000)) return trains(2, 20)
  }
  return row.railType === 0 || row.railType === 3 ? trains(1, 4) : null
}

const SD_PORT: Line = [[32.53, 15.58], [33.44, 16.69], [33.99, 17.70], [35.0, 18.0], [36.32, 18.33], [37.22, 19.62]]
const SD_WEST: Line = [[32.53, 15.58], [33.52, 14.40], [33.62, 13.55], [32.66, 13.17], [30.22, 13.18]]
const SD_NORTH: Line = [[32.53, 15.58], [33.44, 16.69], [33.99, 17.70], [33.32, 19.53], [32.2, 20.7], [31.35, 21.80]]
const SD_EAST: Line = [[33.62, 13.55], [35.38, 14.04], [36.40, 15.45], [36.4, 16.8], [36.32, 18.33], [37.22, 19.62]]
function classifySd(row: RailwayRow) {
  if (row.railType !== 0 && row.railType !== 3) return null
  if (row.usage === 2) return trains(0, 1)
  if (nearPolyline(row, SD_PORT, 18_000)) return trains(1, 3)
  if (nearPolyline(row, SD_WEST, 18_000)) return trains(1, 2)
  if (nearPolyline(row, SD_NORTH, 18_000) || nearPolyline(row, SD_EAST, 18_000)) return trains(1, 1)
  return trains(1, 1)
}

const TZ_SGR: Line = [[39.28, -6.82], [38.95, -6.85], [38.65, -6.75], [37.661, -6.821], [37.00, -6.83], [36.30, -6.40], [35.742, -6.173], [35.78, -5.97]]
const TZ_TAZARA: Line = [[39.28, -6.82], [38.30, -7.30], [37.60, -7.75], [36.68, -8.13], [35.83, -8.86], [34.83, -8.85], [33.46, -8.91], [32.77, -9.30]]
function classifyTz(row: RailwayRow) {
  if (row.railType !== 0) return null
  if (row.usage === 2) return trains(0, 4)
  if (nearPolyline(row, TZ_SGR, 12_000)) return trains(10, 20)
  if (nearPolyline(row, TZ_TAZARA, 12_000)) return trains(3, 12)
  return trains(2, 6)
}

export const AFRICA_RAILWAY_PROXY_SPECS: readonly RailwayProxySpec[] = [
  { iso2: 'CD', bbox: [-13.5, 12.0, 5.5, 31.5], sourceId: SOURCE_ID_CD_NATIONAL_RAILWAY, classify: classifyCd },
  { iso2: 'DZ', bbox: [18.9, -8.7, 37.1, 12.0], sourceId: SOURCE_ID_DZ_NATIONAL_RAILWAY, classify: classifyDz },
  { iso2: 'EG', bbox: [22.0, 24.7, 31.7, 36.9], sourceId: SOURCE_ID_EG_NATIONAL_RAILWAY, classify: classifyEg },
  { iso2: 'ET', bbox: [3.3, 32.9, 15.0, 48.1], sourceId: SOURCE_ID_ET_NATIONAL_RAILWAY, classify: classifyEt },
  { iso2: 'KE', bbox: [-4.7, 33.9, 5.5, 41.9], sourceId: SOURCE_ID_KE_NATIONAL_RAILWAY, classify: classifyKe },
  { iso2: 'MA', bbox: [20.7, -17.3, 36.1, -0.9], sourceId: SOURCE_ID_MA_NATIONAL_RAILWAY, classify: classifyMa },
  { iso2: 'NG', bbox: [4.0, 2.7, 13.9, 14.7], sourceId: SOURCE_ID_NG_NATIONAL_RAILWAY, classify: classifyNg },
  { iso2: 'SD', bbox: [8.7, 21.8, 22.2, 38.6], sourceId: SOURCE_ID_SD_NATIONAL_RAILWAY, classify: classifySd },
  { iso2: 'TZ', bbox: [-11.8, 29.3, -0.9, 40.5], sourceId: SOURCE_ID_TZ_NATIONAL_RAILWAY, classify: classifyTz },
]
