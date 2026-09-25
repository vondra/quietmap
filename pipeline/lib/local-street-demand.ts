/** S2p local-street background plus routed building demand, calibrated on training squares only. */

import { readFileSync } from 'node:fs'
import type { StreetDemand } from './service-tree-flow.js'

export interface LocalStreetParameters {
  residentialUrban: number; unclassifiedUrban: number; rural: number
  throughFactor: number; demandScale: number
}
export const LOCAL_STREET_PARAMETERS: LocalStreetParameters = JSON.parse(
  readFileSync(new URL('./local-street-demand.json', import.meta.url), 'utf8'),
).parameters

export function localStreetAadt(
  roadClass: number, builtUp: number, demand: StreetDemand, parameters = LOCAL_STREET_PARAMETERS,
): number {
  // Living streets share the residential cell; unknown built-up uses the pooled rural cell.
  const background = builtUp === 2
    ? roadClass === 9 ? parameters.unclassifiedUrban : parameters.residentialUrban : parameters.rural
  return background * (demand.through ? parameters.throughFactor : 1) + parameters.demandScale * demand.trips
}
