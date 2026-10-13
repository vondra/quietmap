// The click's visible rows and the Rest row that holds everything else.
import type { Contributor, LayerLevels } from '../../types/noise'

/** A row the list does not show: a contributor, or the aircraft layer as a whole. */
export interface HiddenRow {
  contributor?: Contributor
  layer?: LayerLevels
}

/** The share a row needs to be shown, and how many rows the list shows at least and
 *  at most. */
export const SHOWN_SHARE_MIN = 0.01
export const SHOWN_ROWS_MIN = 3
export const SHOWN_ROWS_MAX = 10

/** Which rows, in the list's order, it shows: before the final update gives their shares
 *  (`undefined`), the first ten; then the first three and every other meeting the share cutoff,
 *  ten at most. */
export function shownRows(shares: (number | undefined)[]): boolean[] {
  const known = shares.every(share => share !== undefined)
  let shown = 0
  return shares.map((share, row) => {
    const show = shown < SHOWN_ROWS_MAX
      && (!known || row < SHOWN_ROWS_MIN || (share as number) >= SHOWN_SHARE_MIN)
    if (show) shown += 1
    return show
  })
}

/** How many sources the last row holds; null when they make no sound (no row). */
export function restSources(layers: LayerLevels[], hidden: HiddenRow[]): number | null {
  const ground = layers.filter(layer => layer.source_type !== 'aircraft')
  const audible = ground.some(layer => layer.unlisted?.lden != null)
    || hidden.some(row => (row.contributor ? row.contributor.received.lden : row.layer?.lden) != null)
  if (!audible) return null
  return ground.reduce((sum, layer) => sum + (layer.unlisted_sources ?? 0), 0) + hidden.length
}
