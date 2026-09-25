/** Holdout rule v1: every fifth z9 square, spatially blocked, keeps its traffic counts out of fits and scored canaries. */

import { createHash } from 'node:crypto'

/** `int(sha256("qm-holdout-v1/{x}/{y}")[:8], 16) % 5 == 0`, fixed 2026-09-24 before any fitting
 *  (Barcelona 259/191 is a holdout square, Prague 276/173 a training square). */
export function isCountHoldoutSquare(x: number, y: number): boolean {
  return Number.parseInt(createHash('sha256').update(`qm-holdout-v1/${x}/${y}`).digest('hex').slice(0, 8), 16) % 5 === 0
}

/** Set to 1 only for a canary scored against held-out counts: no measured count is written in a holdout square. */
export const EXCLUDE_HOLDOUT_COUNTS_ENVIRONMENT = 'QM_EXCLUDE_HOLDOUT_COUNTS'

/** Whether writes to this prepared roads file must withhold measured counts under the switch. */
export function withholdsMeasuredCounts(arrowPath: string, environment: NodeJS.ProcessEnv = process.env): boolean {
  if (environment[EXCLUDE_HOLDOUT_COUNTS_ENVIRONMENT] !== '1') return false
  const square = /(?:^|\/)z9\/(\d+)\/(\d+)\/[^/]+$/.exec(arrowPath)
  if (!square) throw new Error(`${EXCLUDE_HOLDOUT_COUNTS_ENVIRONMENT}=1 needs a z9/x/y path, got ${arrowPath}`)
  return isCountHoldoutSquare(Number(square[1]), Number(square[2]))
}

/** Source observations are withheld before matching and aggregation, not just at their destination row. */
export function excludesHoldoutCounts(): boolean {
  return process.env[EXCLUDE_HOLDOUT_COUNTS_ENVIRONMENT] === '1'
}

function sourceSquare(latitude: number, longitude: number): [number, number] {
  if (!Number.isFinite(latitude) || !Number.isFinite(longitude) || Math.abs(latitude) > 90 || Math.abs(longitude) > 180) {
    throw new Error('invalid count location')
  }
  const lat = Math.max(-85.05112878, Math.min(85.05112878, latitude))
  return [Math.floor((longitude + 180) / 360 * 512) % 512,
    Math.max(0, Math.min(511, Math.floor((1 - Math.asinh(Math.tan(lat * Math.PI / 180)) / Math.PI) * 256)))]
}

export function withholdsCountPoint(latitude: number, longitude: number): boolean {
  return excludesHoldoutCounts() && isCountHoldoutSquare(...sourceSquare(latitude, longitude))
}

/** Conservative section exclusion: reserve each edge's tile rectangle, including its interior.
 * Disconnected parts must be passed separately. A dateline edge follows its shorter longitude arc. */
export function withholdsCountLine(coordinates: readonly (readonly [number, number])[]): boolean {
  if (!excludesHoldoutCounts()) return false
  if (!coordinates.length) throw new Error('count section has no location')
  const squares = coordinates.map(([lon, lat]) => sourceSquare(lat, lon))
  for (let i = 0; i < squares.length; i++) {
    const [x0, y0] = squares[Math.max(0, i - 1)]
    let [x1, y1] = squares[i]
    if (x1 - x0 > 256) x1 -= 512
    else if (x0 - x1 > 256) x1 += 512
    for (let x = Math.min(x0, x1); x <= Math.max(x0, x1); x++) {
      for (let y = Math.min(y0, y1); y <= Math.max(y0, y1); y++) {
        if (isCountHoldoutSquare((x + 512) % 512, y)) return true
      }
    }
  }
  return false
}

/** GeoJSON coordinate nesting, preserving separate line parts instead of inventing connectors. */
export function withholdsCountGeometry(coordinates: unknown): boolean {
  if (!excludesHoldoutCounts()) return false
  if (!Array.isArray(coordinates) || !coordinates.length) throw new Error('count geometry has no location')
  if (typeof coordinates[0] === 'number') return withholdsCountPoint(Number(coordinates[1]), coordinates[0])
  if (Array.isArray(coordinates[0]) && typeof coordinates[0][0] === 'number') {
    return withholdsCountLine(coordinates as Array<[number, number]>)
  }
  return coordinates.some(withholdsCountGeometry)
}
