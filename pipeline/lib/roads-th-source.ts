/** Parse the pinned Thailand DRR AADT table and preserve its published vehicle classes. */

import { parse } from 'csv-parse/sync'
import { readPinnedRoadSource } from './pinned-road-source.js'
import type { RoadLoaderArguments } from './road-loader-cli.js'

const DRR_SHA256 = '1c56711b4b3f4cc595a2db07f31a0e953d8a526219301e1fc21156bce7992e83'
const CLASS_COLUMNS = ['MC', 'SV', 'SVT', 'TB2', 'TB3', 'T4', 'ART3', 'ART4', 'ART5', 'ART6', 'BD', 'DRT'] as const

type DrrClassColumn = (typeof CLASS_COLUMNS)[number]
export interface ThailandDrrRecord {
  roadCode: string
  total: number
  classes: Readonly<Record<DrrClassColumn, number>>
}

export interface ThailandDrrSource {
  records: ReadonlyMap<string, ThailandDrrRecord>
  sourceRows: number
  unavailableTrafficSkipped: number
  invalidClassCountsSkipped: number
  supersededRecords: number
}

function nonnegativeNumber(value: unknown): number | null {
  if (value === null || value === undefined || String(value).trim() === '') return 0
  const number = Number(value)
  return Number.isFinite(number) && number >= 0 ? number : null
}

export function parseThailandDrrSource(text: string): ThailandDrrSource {
  const rows = parse(text, { columns: true, skip_empty_lines: true, bom: true,
    relax_column_count: false, record_delimiter: ['\r\n', '\n', '\r'] }) as Record<string, unknown>[]
  const records = new Map<string, ThailandDrrRecord>()
  let unavailableTrafficSkipped = 0, invalidClassCountsSkipped = 0, supersededRecords = 0
  for (const row of rows) {
    const roadCode = String(row.road_code ?? '').trim()
    const total = Number(row.sum_AADT)
    if (!roadCode || !Number.isFinite(total) || total <= 0) { unavailableTrafficSkipped++; continue }
    const values = CLASS_COLUMNS.map(column => nonnegativeNumber(row[column]))
    if (values.some(value => value === null)) { invalidClassCountsSkipped++; continue }
    const classes = Object.fromEntries(CLASS_COLUMNS.map((column, index) => [column, values[index]!])) as Record<DrrClassColumn, number>
    const traffic = thailandDrrTraffic({ roadCode, total, classes })
    if (traffic.light + traffic.medium + traffic.heavy + traffic.moto === 0) {
      invalidClassCountsSkipped++
      continue
    }
    if (records.has(roadCode)) supersededRecords++
    records.set(roadCode, { roadCode, total, classes })
  }
  if (rows.length === 0 || records.size === 0) throw new Error('Thailand DRR source has no usable AADT records')
  return { records, sourceRows: rows.length, unavailableTrafficSkipped,
    invalidClassCountsSkipped, supersededRecords }
}

export function thailandDrrTraffic(record: ThailandDrrRecord) {
  const c = record.classes
  return {
    light: Math.round(c.SV + c.SVT),
    medium: Math.round(c.TB2 + c.BD + c.DRT),
    heavy: Math.round(c.TB3 + c.T4 + c.ART3 + c.ART4 + c.ART5 + c.ART6),
    moto: Math.round(c.MC),
  }
}

export function loadThailandDrrSource(options: RoadLoaderArguments): ThailandDrrSource {
  return parseThailandDrrSource(readPinnedRoadSource(options, 'th/drr-aadt-2024.csv', DRR_SHA256).toString('utf8'))
}
