// Vocabulary shared by the popup's contributor rows: layer and class labels, distances, rows.
import type { ReactNode } from 'react'
import type { Contributor } from '../../types/noise'

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

/** The contributor's name, else its class in words (a builder falls back to the raw class name). */
export function contributorLabel(c: Contributor): string {
  const cls = contributorClass(c)
  if (c.name && c.name !== cls) return c.name
  return cls ? subtypeLabel(c.source_type, cls) : SOURCE_LABELS[c.source_type] ?? c.source_type
}

/** Any display field as text, whatever its shape. */
export function fieldText(value: unknown): string {
  if (Array.isArray(value)) return value.map(fieldText).join(', ')
  if (value !== null && typeof value === 'object') return JSON.stringify(value)
  return String(value)
}

export function formatDist(m: number): string {
  if (m === 0) return 'overhead'
  if (m < 1000) return `${m} m`
  return `${(m / 1000).toFixed(1)} km`
}

export function lineRow(label: ReactNode, value: ReactNode, muted?: boolean) {
  return (
    <div className={`flex justify-between gap-3 ${muted ? 'text-muted-foreground/40' : ''}`}>
      <span className="shrink-0">{label}</span>
      <span className={`text-right ${muted ? '' : 'text-foreground'}`}>{value}</span>
    </div>
  )
}

/** Period labels with their hours, for tables and tooltips. */
export const PERIOD_LABELS_DETAIL = [
  'Day (07–19)',
  'Evening (19–23)',
  'Night (23–07)',
] as const
