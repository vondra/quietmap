/** Dev1's operator-published railway proxy tiers for seven Eurasian countries plus KR no-source. */

import {
  SOURCE_ID_IQ_NATIONAL_RAILWAY, SOURCE_ID_IR_NATIONAL_RAILWAY,
  SOURCE_ID_KR_NATIONAL_RAILWAY, SOURCE_ID_KZ_NATIONAL_RAILWAY,
  SOURCE_ID_RU_NATIONAL_RAILWAY, SOURCE_ID_TR_NATIONAL_RAILWAY,
  SOURCE_ID_UA_NATIONAL_RAILWAY, SOURCE_ID_UZ_NATIONAL_RAILWAY,
} from './source-ids.generated.js'
import {
  inCentreBox, inCoordinateBox, nearAnyPolyline, nearPolyline, trains,
  type RailwayProxySpec,
} from './railway-proxy-rules.js'
import type { RailwayRow } from './railways-arrow.js'

type Line = ReadonlyArray<readonly [number, number]>

const IQ_BAGHDAD_BASRA: Line = [[44.36, 33.31], [44.66, 32.60], [44.93, 31.99], [45.30, 31.32], [46.26, 31.05], [47.78, 30.51]]
function classifyIq(row: RailwayRow) {
  if (row.railType !== 0) return null
  return nearPolyline(row, IQ_BAGHDAD_BASRA, 12_000) ? trains(3, 4) : trains(1, 1)
}

const IR_MASHHAD: Line = [[51.42, 35.70], [53.39, 35.58], [54.98, 36.42], [56.40, 36.30], [58.80, 36.22], [59.57, 36.30]]
const IR_ISFAHAN_SHIRAZ: Line = [[51.42, 35.70], [50.88, 34.64], [51.45, 33.98], [51.67, 32.65], [52.20, 31.20], [52.53, 29.59]]
const IR_TABRIZ: Line = [[51.42, 35.70], [50.00, 36.27], [48.49, 36.68], [47.70, 37.45], [46.29, 38.08]]
const IR_BANDAR_ABBAS: Line = [[51.42, 35.70], [50.88, 34.64], [51.90, 32.00], [54.37, 31.90], [55.40, 31.60], [55.95, 29.00], [56.28, 27.18]]
function classifyIr(row: RailwayRow) {
  if (row.railType === 1 || row.railType === 2) {
    return inCentreBox(row, 35.70, 51.42, 0.45) ? trains(350, 0) : trains(80, 0)
  }
  if (row.railType !== 0) return null
  if (row.usage === 2) return trains(0, 5)
  if (nearPolyline(row, IR_MASHHAD, 9_000)) return trains(15, 8)
  if (nearPolyline(row, IR_ISFAHAN_SHIRAZ, 9_000)) return trains(8, 10)
  if (nearPolyline(row, IR_TABRIZ, 9_000)) return trains(5, 8)
  if (nearPolyline(row, IR_BANDAR_ABBAS, 9_000)) return trains(3, 10)
  return trains(3, 5)
}

const KZ_COAL: Line = [[76.97, 52.29], [75.37, 51.67], [73.10, 49.80]]
const KZ_ALMATY_ASTANA: Line = [[76.92, 43.24], [74.98, 46.84], [73.10, 49.80], [71.43, 51.13]]
function classifyKz(row: RailwayRow) {
  if (row.railType === 1 || row.railType === 2) {
    if (row.railType === 1 && inCoordinateBox(row, [43.18, 76.80, 43.32, 77.02])) return trains(80, 0)
    if (row.railType === 2 && inCoordinateBox(row, [50.95, 71.25, 51.25, 71.70])) return trains(50, 0)
    return trains(60, 0)
  }
  if (row.railType !== 0) return null
  if (row.usage === 2) return trains(0, 6)
  if (nearPolyline(row, KZ_COAL, 20_000)) return trains(2, 30)
  if (nearPolyline(row, KZ_ALMATY_ASTANA, 18_000)) return trains(8, 20)
  return row.usage === 1 ? trains(1, 4) : trains(2, 10)
}

const RU_TRANSSIB: Line = [
  [37.62, 55.75], [39.87, 57.63], [49.66, 58.60], [56.25, 58.01], [60.61, 56.84],
  [65.53, 57.15], [73.37, 54.99], [82.92, 55.03], [92.85, 56.01], [104.28, 52.29],
  [107.58, 51.83], [113.50, 52.03], [119.74, 53.74], [123.95, 54.00], [128.40, 50.92],
  [135.07, 48.48], [131.94, 43.80], [131.89, 43.12],
]
const RU_MOSCOW_SPB: Line = [[37.62, 55.75], [35.91, 56.86], [33.27, 57.88], [30.31, 59.94]]
const RU_BAM: Line = [[98.00, 55.93], [101.61, 56.15], [105.77, 56.79], [109.32, 55.63], [124.72, 55.15], [137.00, 50.55], [140.25, 49.09]]
const RU_KUZBASS: Line = [[84.95, 55.72], [86.09, 55.35], [87.12, 53.79], [88.07, 53.69]]
const RU_PORTS = [[59.3, 27.7, 60.0, 28.9], [44.4, 37.4, 45.0, 38.1], [42.5, 132.4, 43.2, 133.3]] as const
const RU_SUBURBAN = [
  [55.75, 37.62, 0.55, 150, 5], [59.94, 30.31, 0.45, 130, 8],
  ...[
    [55.03, 82.92], [56.84, 60.61], [55.79, 49.12], [56.33, 44.00], [56.01, 92.85],
    [55.16, 61.40], [53.20, 50.15], [54.74, 55.97], [47.24, 39.71], [45.04, 38.98],
    [54.99, 73.37], [51.66, 39.20], [58.01, 56.25], [48.71, 44.51],
  ].map(([latitude, longitude]) => [latitude, longitude, 0.20, 60, 8]),
] as const
function ruSuburban(row: RailwayRow) {
  for (const [latitude, longitude, half, passenger, freight] of RU_SUBURBAN) {
    if (inCentreBox(row, latitude, longitude, half)) return trains(passenger, freight)
  }
  return null
}
function classifyRu(row: RailwayRow) {
  if (row.railType === 1 || row.railType === 2) return trains(200, 0)
  if (row.railType !== 0) return null
  if (row.usage === 2) return trains(0, 6)
  if (nearPolyline(row, RU_MOSCOW_SPB, 8_000)) return trains(35, 15)
  if (nearPolyline(row, RU_KUZBASS, 15_000)) return trains(12, 120)
  if (nearPolyline(row, RU_TRANSSIB, 12_000)) return trains(30, 110)
  if (nearPolyline(row, RU_BAM, 15_000)) return trains(6, 50)
  if (RU_PORTS.some(box => inCoordinateBox(row, box))) return trains(8, 70)
  return ruSuburban(row) ?? (row.usage === 1 ? trains(4, 12) : trains(12, 45))
}

const TR_MARMARAY: Line = [[28.79, 41.00], [28.85, 41.00], [28.95, 40.99], [28.98, 41.01], [29.01, 41.02], [29.06, 41.00], [29.16, 40.96], [29.31, 40.86], [29.43, 40.79]]
const TR_IZBAN: Line = [[26.97, 38.80], [27.03, 38.65], [27.14, 38.46], [27.15, 38.42], [27.25, 38.30], [27.37, 37.95]]
const TR_YHT: readonly Line[] = [
  [[32.86, 39.93], [31.80, 39.80], [30.52, 39.78], [29.98, 40.05], [29.92, 40.77], [29.55, 40.85], [29.25, 40.88]],
  [[32.86, 39.93], [32.15, 39.58], [32.30, 38.70], [32.48, 37.87]],
  [[32.86, 39.93], [33.51, 39.84], [34.80, 39.82], [36.00, 39.78], [37.02, 39.75]],
]
const TR_ORE: readonly Line[] = [
  [[38.12, 39.37], [38.31, 38.36], [37.00, 37.00], [36.17, 36.58]],
  [[31.80, 41.45], [32.63, 41.20]],
]
const TR_MAIN: readonly Line[] = [
  [[32.86, 39.93], [31.40, 39.20], [30.55, 38.76], [29.40, 38.68], [27.43, 38.61], [27.14, 38.42]],
  [[32.86, 39.93], [33.51, 39.84], [34.50, 39.10], [35.48, 38.73], [36.30, 39.30], [37.02, 39.75]],
  [[32.48, 37.87], [33.60, 37.90], [34.68, 37.97], [35.10, 37.40], [35.32, 37.00], [34.63, 36.81]],
  [[35.32, 37.00], [36.25, 37.07], [37.38, 37.07]],
  [[30.52, 39.78], [29.98, 39.42], [27.89, 39.65], [27.43, 38.61]],
  [[28.95, 41.05], [28.00, 41.28], [26.56, 41.68]],
  [[37.02, 39.75], [39.49, 39.75], [41.27, 39.90], [43.10, 40.60]],
  [[38.31, 38.36], [40.24, 37.91], [41.13, 37.88]],
  [[29.25, 40.88], [30.52, 39.78], [32.86, 39.93]],
]
function classifyTr(row: RailwayRow) {
  const istanbul = inCoordinateBox(row, [40.80, 28.45, 41.35, 29.45])
  if (row.railType === 4) return trains(100, 0)
  if (row.railType === 1) return trains(istanbul ? 350 : 250, 0)
  if (row.railType === 2) return trains(istanbul ? 500 : 400, 0)
  if (row.railType !== 0) return null
  if (row.usage === 2) return trains(0, 5)
  if (nearPolyline(row, TR_MARMARAY, 6_000)) return trains(400, 0)
  if (nearPolyline(row, TR_IZBAN, 6_000)) return trains(150, 4)
  if (nearAnyPolyline(row, TR_YHT, 8_000)) return trains(40, 0)
  if (nearAnyPolyline(row, TR_ORE, 12_000)) return trains(6, 20)
  if (nearAnyPolyline(row, TR_MAIN, 12_000)) return trains(20, 12)
  return row.usage === 1 ? trains(1, 3) : trains(8, 6)
}

const UA_MAJOR_TRAMS = [[50.45, 30.52, 0.28], [49.99, 36.23, 0.20], [46.48, 30.72, 0.20], [49.84, 24.03, 0.15], [48.46, 35.04, 0.20], [47.91, 33.39, 0.30]] as const
const UA_FAST_TRAMS = [[47.91, 33.39, 0.30], [50.46, 30.45, 0.28]] as const
const inAnyCentreBox = (row: RailwayRow, boxes: ReadonlyArray<readonly [number, number, number]>) =>
  boxes.some(([latitude, longitude, half]) => inCentreBox(row, latitude, longitude, half))
function classifyUa(row: RailwayRow) {
  if (row.railType === 1) return trains(inAnyCentreBox(row, UA_MAJOR_TRAMS) ? 180 : 90, 0)
  if (row.railType === 2) return trains(inAnyCentreBox(row, UA_FAST_TRAMS) ? 150 : 120, 0)
  if (row.railType === 3) return trains(2, 1)
  if (row.railType !== 0) return null
  if (row.usage === 2) return trains(0, 6)
  return row.usage === 1 ? trains(5, 8) : trains(20, 15)
}

const UZ_SILK: Line = [[69.24, 41.31], [67.84, 40.12], [66.96, 39.65], [65.38, 40.10], [64.42, 39.77]]
const UZ_FERGHANA: Line = [[69.24, 41.31], [70.14, 41.02], [71.10, 40.87], [71.67, 40.99], [72.34, 40.78]]
const UZ_NORTHWEST: Line = [[64.42, 39.77], [62.20, 40.10], [60.63, 41.55], [59.61, 42.46], [58.54, 43.06]]
const UZ_TERMEZ: Line = [[66.96, 39.65], [65.79, 38.86], [67.28, 37.22]]
function classifyUz(row: RailwayRow) {
  if (row.railType === 1 || row.railType === 2) return trains(200, 0)
  if (row.railType !== 0) return null
  if (row.usage === 2) return trains(0, 6)
  if (nearPolyline(row, UZ_SILK, 12_000)) return trains(8, 14)
  if (nearPolyline(row, UZ_FERGHANA, 12_000)) return trains(5, 11)
  if (nearPolyline(row, UZ_NORTHWEST, 15_000)) return trains(5, 11)
  if (nearPolyline(row, UZ_TERMEZ, 12_000)) return trains(4, 12)
  return row.usage === 1 ? trains(2, 5) : trains(4, 8)
}

export const EURASIA_RAILWAY_PROXY_SPECS: readonly RailwayProxySpec[] = [
  { iso2: 'IQ', bbox: [29.0, 38.7, 37.4, 48.6], sourceId: SOURCE_ID_IQ_NATIONAL_RAILWAY, classify: classifyIq },
  { iso2: 'IR', bbox: [25.0, 44.0, 39.8, 63.5], sourceId: SOURCE_ID_IR_NATIONAL_RAILWAY, classify: classifyIr },
  // Dev1's KR file is only a legacy-stamp heal: no open source exists and fresh z9 must stay unstamped.
  { iso2: 'KR', bbox: [33.0, 124.5, 39.0, 132.0], sourceId: SOURCE_ID_KR_NATIONAL_RAILWAY, classify: null },
  { iso2: 'KZ', bbox: [40.0, 46.0, 56.0, 88.0], sourceId: SOURCE_ID_KZ_NATIONAL_RAILWAY, classify: classifyKz },
  { iso2: 'RU', bbox: [41.0, 19.0, 82.0, 180.0], sourceId: SOURCE_ID_RU_NATIONAL_RAILWAY, classify: classifyRu },
  { iso2: 'TR', bbox: [35.8, 25.6, 42.2, 44.8], sourceId: SOURCE_ID_TR_NATIONAL_RAILWAY, classify: classifyTr },
  { iso2: 'UA', bbox: [44.0, 22.0, 52.5, 40.5], sourceId: SOURCE_ID_UA_NATIONAL_RAILWAY, classify: classifyUa },
  { iso2: 'UZ', bbox: [37.2, 55.9, 45.6, 73.2], sourceId: SOURCE_ID_UZ_NATIONAL_RAILWAY, classify: classifyUz },
]
