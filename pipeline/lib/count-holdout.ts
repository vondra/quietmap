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
