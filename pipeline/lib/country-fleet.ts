/** Derive the service-tree fleet from the single cited country-fleet.json model. */

import { readFileSync } from 'node:fs'

export interface CountryFleet { motoTrafficShare: number; tripsPerDwelling: number }
interface FleetPrior {
  continent: string
  fleet?: { moto_fleet_share: number }
  moto_traffic_share_override?: number
  vehicle_trips_per_occupied_dwelling?: number
}
interface FleetBand { moto_traffic_share: number; vehicle_trips_per_occupied_dwelling: number }
const data = JSON.parse(readFileSync(new URL('./country-fleet.json', import.meta.url), 'utf8')) as {
  world: FleetBand & { medium_share: number; heavy_share: number }; continents: Record<string, FleetBand>; countries: Record<string, FleetPrior>
}
export const LOCAL_MEDIUM_SHARE = data.world.medium_share
export const LOCAL_HEAVY_SHARE = data.world.heavy_share
export const WORLD_FLEET: CountryFleet = {
  motoTrafficShare: data.world.moto_traffic_share,
  tripsPerDwelling: data.world.vehicle_trips_per_occupied_dwelling,
}
// Ownership→flow calibration: TH/Asia .4, CZ/EU and recreational NA/Oceania .15,
// BR/South America .2. African registrations omit informal fleets; use its band.
const ownershipFactors: Readonly<Record<string, number>> = {
  Asia: 0.4, Europe: 0.15, 'North America': 0.15, 'South America': 0.2, Oceania: 0.15,
}
const clamp = (value: number, low: number, high: number) => Math.min(Math.max(value, low), high)
const fleets = new Map<string, CountryFleet>()
for (const [iso, row] of Object.entries(data.countries)) {
  const band = data.continents[row.continent]
  const factor = ownershipFactors[row.continent]
  const moto = row.moto_traffic_share_override ??
    (row.fleet?.moto_fleet_share !== undefined && factor !== undefined
      ? clamp(factor * row.fleet.moto_fleet_share, 0.01, 0.45)
      : (band ?? data.world).moto_traffic_share)
  // OECD HM1-1 stock occupancy: survey households ×0.92; WORLD already includes it.
  const trips = row.vehicle_trips_per_occupied_dwelling !== undefined
    ? row.vehicle_trips_per_occupied_dwelling * 0.92
    : band ? band.vehicle_trips_per_occupied_dwelling * 0.92 : WORLD_FLEET.tripsPerDwelling
  fleets.set(iso, { motoTrafficShare: Number(clamp(moto, 0.01, 0.45).toFixed(3)),
    tripsPerDwelling: Number(clamp(trips, 0.8, 6).toFixed(2)) })
}

export function fleetForIso(iso: string | undefined): CountryFleet {
  return (iso === undefined ? undefined : fleets.get(iso)) ?? WORLD_FLEET
}
