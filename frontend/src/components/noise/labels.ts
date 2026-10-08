// What a contributor is called in the popup: layer and class words, the contributor's label, and
// what the aircraft layer is made of.
import type { AircraftKind, Contributor } from '../../types/noise'

export const SOURCE_LABELS: Record<string, string> = {
  road: 'Roads',
  railway: 'Railways',
  aircraft: 'Aircraft',
  industrial: 'Industrial',
  building: 'Buildings',
  ship: 'Ships',
}

const SUBTYPE_LABELS: Record<string, Record<string, string>> = {
  road: {
    motorway: 'Motorway',
    trunk: 'Trunk road',
    primary: 'Primary road',
    secondary: 'Secondary road',
    tertiary: 'Tertiary road',
    residential: 'Local road',
    living_street: 'Living street',
    unclassified: 'Minor road',
    service: 'Service road',
    motorway_link: 'Motorway ramp',
    trunk_link: 'Trunk road ramp',
    primary_link: 'Primary road ramp',
  },
  railway: {
    freight_corridor: 'Freight railway',
    passenger: 'Railway',
    tram: 'Tram',
    light_rail: 'Light rail',
    rail: 'Railway',
    narrow_gauge: 'Narrow gauge',
    funicular: 'Funicular',
    heritage: 'Heritage railway',
    horn: 'Level-crossing horn',
    'rail (bridge)': 'Railway (bridge)',
    'tram (bridge)': 'Tram (bridge)',
    'light_rail (bridge)': 'Light rail (bridge)',
    'narrow_gauge (bridge)': 'Narrow gauge (bridge)',
    'funicular (bridge)': 'Funicular (bridge)',
    'heritage (bridge)': 'Heritage railway (bridge)',
    'horn (bridge)': 'Level-crossing horn (bridge)',
  },
  industrial: {
    industrial_area: 'Industrial area',
    quarry: 'Quarry',
    farm: 'Farm',
    factory: 'Factory',
    wastewater: 'Wastewater plant',
    rail_yard: 'Rail yard',
    wind_turbine: 'Wind turbine',
    solar_farm: 'Solar farm',
    substation: 'Substation',
  },
  ship: {
    // Keys ARE the backend names (emission/ships.rs::ShipClass::name).
    large_ships: 'Large ships',
    work_boats: 'Work boats',
    leisure_craft: 'Leisure craft',
  },
  building: {
    // Keys ARE the backend names (source_names.rs::building_type_name) — keep in
    // sync. Leisure sport areas fold into the building layer, so their names live
    // here too (one source of truth for every building-layer label).
    residential_multi: 'Apartments',
    residential_house: 'House',
    commercial: 'Commercial / office',
    food_retail: 'Shop / supermarket',
    restaurant_bar: 'Restaurant / bar',
    warehouse: 'Warehouse / factory',
    education: 'School',
    healthcare: 'Hospital / clinic',
    worship: 'Place of worship',
    church_bells: 'Church bells',
    call_to_prayer: 'Call to prayer',
    people_bar: 'People at a bar',
    people_pub: 'People at a pub',
    people_nightclub: 'People at a nightclub',
    people_biergarten: 'Beer garden guests',
    people_restaurant: 'People at a restaurant',
    people_cafe: 'People at a café',
    people_fast_food: 'People at a fast-food place',
    hotel: 'Hotel',
    garage: 'Garage / parking',
    farm: 'Farm',
    public: 'Public building',
    silent: 'Building',
    padel_court: 'Padel court',
    tennis_court: 'Tennis court',
    ball_court: 'Ball court',
    playground: 'Playground',
    swimming_pool: 'Swimming pool',
    outdoor_seating: 'Outdoor seating',
    stadium: 'Stadium',
    sports_pitch: 'Sports pitch',
    car_park: 'Car park',
    street_parking: 'Street parking',
    motorsport_circuit: 'Race circuit',
    motorsport_motocross: 'Motocross track',
    motorsport_kart: 'Kart track',
    motorsport_speedway: 'Speedway track',
    motorsport_trial: 'Trial park',
    motorsport: 'Motorsport',
    shooting: 'Shooting range',
    shooting_rifle: 'Rifle range',
    shooting_pistol: 'Pistol range',
    shooting_shotgun: 'Shotgun range',
    default: 'Building',
  },
  aircraft: { mixed: 'Aircraft', aircraft: 'Aircraft', airport_traffic: 'Airport ground operations' },
}

/** A class in words. What follows a colon names the one source, not its class (the airport of
 *  `airport_traffic:LKPR`). */
export function subtypeLabel(sourceType: string, subtype: string): string {
  return SUBTYPE_LABELS[sourceType]?.[subtype.split(':')[0]] || subtype.replace(/_/g, ' ')
}

/** The class a contributor's label falls back to: road class, rail type, building or site type. */
export function contributorClass(c: Contributor): string {
  for (const key of ['road_class', 'rail_type', 'building_type', 'source_type']) {
    const value = c.metadata?.[key]
    if (typeof value === 'string' && value) return value
  }
  return c.subtype ?? ''
}

/** What a reference alone is the number of: a bare "2404" tells a visitor nothing, "Road 2404" does
 *  and fits the row's column, where "Secondary road 240" does not (the class stands in the row's
 *  detail). A line keeps its class: "Tram 22", "Railway 011". */
const REFERENCE_NOUNS: Record<string, string> = { road: 'Road' }

function kindOf(c: Contributor): string {
  const cls = contributorClass(c)
  return cls ? subtypeLabel(c.source_type, cls) : SOURCE_LABELS[c.source_type] ?? c.source_type
}

/** The contributor's name, else its number with what it numbers ("Road 2404"), else its class in
 *  words. */
export function contributorLabel(c: Contributor): string {
  const kind = kindOf(c)
  if (!c.name || c.name === contributorClass(c)) return kind
  const referenceOnly = c.name === c.metadata?.ref && !c.metadata?.name
  return referenceOnly ? `${REFERENCE_NOUNS[c.source_type] ?? kind} ${c.name}` : c.name
}

/** Whether the label already says the contributor's class ("Tertiary road", "Tram 22"), so its
 *  detail need not repeat it. */
export function labelNamesClass(c: Contributor): boolean {
  const kind = kindOf(c)
  const label = contributorLabel(c)
  return contributorClass(c) === '' || label === kind || label === `${kind} ${c.name}`
}

const AIRCRAFT_KIND_LABELS: Record<AircraftKind, string> = {
  airliners: 'Airliners',
  regional_business_jets: 'Regional and business jets',
  propeller: 'Propeller aircraft',
  helicopters: 'Helicopters',
  ground: 'Airport ground operations',
}

/** The aircraft layer's kinds, largest first, in words with their shares. */
export function aircraftKindShares(kinds: Partial<Record<AircraftKind, number>>): [string, number][] {
  return (Object.entries(kinds) as [AircraftKind, number][])
    .sort((a, b) => b[1] - a[1])
    .map(([kind, share]) => [AIRCRAFT_KIND_LABELS[kind] ?? kind, share])
}
