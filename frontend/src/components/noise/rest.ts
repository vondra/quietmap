// The list's last row: everything it does not name, together: the rows past the list's length (the
// aircraft layer among them when under 0 dB) and what the popup leaves out of each ground layer (its
// contributors cut from the thirty or under 0 dB). Pure TypeScript, so it has a dependency-free unit
// test.
import type { Contributor, LayerLevels } from '../../types/noise'

/** A row the list does not show: a contributor, or the aircraft layer as a whole. */
export interface HiddenRow {
  contributor?: Contributor
  layer?: LayerLevels
}

/** How many sources the last row holds; null when they make no sound (no row). */
export function restSources(layers: LayerLevels[], hidden: HiddenRow[]): number | null {
  const ground = layers.filter(layer => layer.source_type !== 'aircraft')
  const audible = ground.some(layer => layer.unlisted?.lden != null)
    || hidden.some(row => (row.contributor ? row.contributor.received.lden : row.layer?.lden) != null)
  if (!audible) return null
  return ground.reduce((sum, layer) => sum + (layer.unlisted_sources ?? 0), 0) + hidden.length
}
