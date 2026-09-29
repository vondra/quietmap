/** S2p local-street background plus routed building demand, calibrated on training squares only. */

import { readFileSync } from 'node:fs'
import type { StreetDemand } from './service-tree-flow.js'

export interface LocalStreetParameters {
  residentialUrban: number; unclassifiedUrban: number; rural: number
  throughFactor: number; demandScale: number; singleTrackFactor: number
}
export const LOCAL_STREET_PARAMETERS: LocalStreetParameters = JSON.parse(
  readFileSync(new URL('./local-street-demand.json', import.meta.url), 'utf8'),
).parameters

export function localStreetAadt(
  roadClass: number, builtUp: number, demand: StreetDemand, parameters = LOCAL_STREET_PARAMETERS,
): number {
  // Living streets share the residential cell; unknown built-up uses the pooled rural cell.
  // Rural connectors carry no through premium (the fitted rural factor is 1.03 ± 0.23),
  // and single-track streets keep only a share of the background.
  const background = builtUp === 2
    ? roadClass === 9 ? parameters.unclassifiedUrban : parameters.residentialUrban : parameters.rural
  const through = demand.through && builtUp === 2 ? parameters.throughFactor : 1
  const narrow = demand.singleTrack ? parameters.singleTrackFactor : 1
  return background * through * narrow + parameters.demandScale * demand.trips
}
