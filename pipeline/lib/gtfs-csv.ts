/** Stream GTFS CSV records and skip unneeded fields of inactive stop-time rows. */

import { createReadStream } from 'node:fs'
import { createInterface } from 'node:readline'
import { resolve } from 'node:path'

/** Parse a single CSV line, handling quoted fields with commas. */
function parseCsvLine(line: string, fieldCount = Infinity): string[] {
  const fields: string[] = []
  let current = ''
  let inQuotes = false
  for (let i = 0; i < line.length; i++) {
    const ch = line[i]
    if (inQuotes) {
      if (ch === '"') {
        if (i + 1 < line.length && line[i + 1] === '"') {
          current += '"'
          i++
        } else {
          inQuotes = false
        }
      } else {
        current += ch
      }
    } else {
      if (ch === '"') {
        inQuotes = true
      } else if (ch === ',') {
        fields.push(current.trim())
        if (fields.length === fieldCount) return fields
        current = ''
      } else {
        current += ch
      }
    }
  }
  fields.push(current.trim())
  return fields
}

/** Read a CSV file with synchronous row consumers and one asynchronous completion. */
async function readCsvFile(
  filePath: string,
  initialize: (headers: string[]) => (line: string) => void,
): Promise<number> {
  const stream = createReadStream(filePath, { encoding: 'utf-8' })
  const lines = createInterface({ input: stream, crlfDelay: Infinity })
  let consume: ((line: string) => void) | undefined
  let count = 0
  try {
    await new Promise<void>((resolve, reject) => {
      const fail = (error: unknown): void => {
        reject(error)
        lines.removeAllListeners('line')
        lines.close()
        stream.destroy()
      }
      lines.once('error', fail)
      lines.once('close', resolve)
      lines.on('line', rawLine => {
        try {
          const line = consume ? rawLine : rawLine.replace(/^\uFEFF/, '')
          if (line.trim() === '') return
          if (!consume) consume = initialize(parseCsvLine(line))
          else { count++; consume(line) }
        } catch (error) { fail(error) }
      })
    })
    return count
  } finally {
    lines.close()
    stream.destroy()
  }
}

/** Visit named CSV rows without retaining the source table. */
export async function readCsvRows(filePath: string, visit: (row: Record<string, string>) => void): Promise<void> {
  await readCsvFile(filePath, headers => line => {
    const fields = parseCsvLine(line)
    const row: Record<string, string> = {}
    for (let i = 0; i < headers.length; i++) row[headers[i]] = fields[i] || ''
    visit(row)
  })
}

/** Count every stop-time row, decoding its remaining fields only for active trips. */
export async function readGtfsStopTimes(
  extractDir: string,
  activeTrips: ReadonlyMap<string, unknown>,
  initialize: (headers: string[], tripIdIndex: number) => (fields: string[]) => void,
): Promise<number> {
  return readCsvFile(resolve(extractDir, 'stop_times.txt'), headers => {
    const tripIdIndex = headers.indexOf('trip_id')
    if (tripIdIndex < 0) throw new Error('stop_times.txt missing trip_id')
    const consume = initialize(headers, tripIdIndex)
    return line => {
      const tripId = parseCsvLine(line, tripIdIndex + 1)[tripIdIndex]
      if (activeTrips.has(tripId)) consume(parseCsvLine(line))
    }
  })
}

/** Collect only the source tables whose callers need every row. */
export async function parseCsvStream(filePath: string): Promise<Record<string, string>[]> {
  const rows: Record<string, string>[] = []
  await readCsvRows(filePath, row => { rows.push(row) })
  return rows
}
