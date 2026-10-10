// A contributor's facts as lines, only what its row and class line do not say: a road's speed,
// traffic and a surface other than asphalt, a railway's speed and trains, a building's floors, height
// and footprint or an area's size and cars, a site's area or a turbine's rated power, a ship cell's
// vessels, an airport's movements; then the source's sound power. A layer without lines of its own
// lists its fields by name, so a newly added layer shows up before it gets its own wording.
import type { ReactNode } from 'react'
import type { Contributor, ContributorMetadata } from '../../../types/noise'
import { fmt, fmtCompact, fmtCount, fmtInt, txtTable, type TableRow } from '../../../utils/formatters'
import { HoverText } from '../../ui/info-tip'
import { subtypeLabel } from '../labels'
import { fieldText, lineRow } from '../shared'
import { railTrafficDescription, railTrafficLabel, roadCategoryEstimated, type RailTraffic } from '../provenance'

function num(m: ContributorMetadata, key: string): number | null {
  const value = m[key]
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

function text(m: ContributorMetadata, key: string): string {
  const value = m[key]
  return typeof value === 'string' ? value : ''
}

const words = (value: string) => value.replace(/_/g, ' ')

/** How a road's or a railway's speed was chosen (the builders' `speed_source`), in words. */
const SPEED_SOURCES: Record<string, string> = {
  osm_posted: 'The posted limit (OpenStreetMap)',
  derestricted: 'No limit: the usual speed on a derestricted motorway',
  graded_transition: 'Between two posted limits',
  country_legal_default: 'No posted limit: the country\'s legal limit',
  tagged_median: 'No posted limit: the median posted on such roads in the country',
  default_by_class: 'No posted limit: the usual speed of the road class',
  rural_free_flow: 'Free-flowing traffic below the limit of a rural road',
  high_speed_default: 'No posted limit: the usual speed of a high-speed line',
  default_by_type: 'No posted limit: the usual speed of the line type',
}

function speedLine(m: ContributorMetadata) {
  const speed = num(m, 'speed_kmh')
  if (speed == null) return null
  const source = text(m, 'speed_source')
  const posted = num(m, 'speed_posted_kmh')
  const why = (SPEED_SOURCES[source] ?? words(source))
    + (posted != null && posted > 0 && posted !== Math.round(speed) ? `\nPosted: ${posted} km/h` : '')
  return lineRow('Speed', <HoverText title={why}>{speed.toFixed(0)} km/h</HoverText>)
}

/** Road surfaces other than asphalt, the reference (the road builder's `SURFACE_NAMES`). */
const SURFACES: Record<string, string> = {
  sett: 'Setts',
  paving_stones: 'Paving stones',
  concrete: 'Concrete',
  unpaved: 'Unpaved',
}

function RoadRows({ m }: { m: ContributorMetadata }) {
  const light = num(m, 'aadt_light') ?? 0
  const medium = num(m, 'aadt_medium') ?? 0
  const heavy = num(m, 'aadt_heavy') ?? 0
  const moto = num(m, 'aadt_moto') ?? 0
  const estimated = { traffic_estimated: num(m, 'traffic_estimated') ?? 0 }
  const total = light + medium + heavy + moto
  const crossSection = num(m, 'cross_section_aadt') ?? 0
  // The headline is the whole road; the classes below are this carriageway's.
  const wholeRoad = crossSection > 0
  const trafficText = txtTable([
    ...(wholeRoad ? [['Whole road, both ways', `${fmtInt(crossSection)}/day`] as [string, string]] : []),
    'This carriageway:',
    ...([['Light', light, 1], ['Medium', medium, 2], ['Heavy', heavy, 4], ['Moto', moto, 8]] as const)
      .map(([label, value, bit]) =>
        [`  ${label}`, roadCategoryEstimated(estimated, bit) ? `${fmtInt(value)} est.` : fmtInt(value)] as [string, string],
      ),
    '',
    'est.: estimated; the others counted',
  ] as TableRow[], 22, 12)
  const surface = SURFACES[text(m, 'surface')]
  const correction = num(m, 'surface_corr_db') ?? 0
  return (
    <>
      {speedLine(m)}
      {lineRow('Traffic', (
        <HoverText title={trafficText}>
          {wholeRoad ? `${fmtCompact(crossSection)}/day` : `${fmtCompact(total)}/day, one way`}
        </HoverText>
      ))}
      {surface && lineRow('Surface', (
        <HoverText title="On the rolling noise of cars, against asphalt (CNOSSOS-EU)">
          {`${surface}, ${fmt(correction)} dB`}
        </HoverText>
      ))}
    </>
  )
}

function railTraffic(m: ContributorMetadata): RailTraffic {
  const category = (kind: 'passenger' | 'freight') => ({
    periods: [
      num(m, `trains_${kind}_day`) ?? 0,
      num(m, `trains_${kind}_evening`) ?? 0,
      num(m, `trains_${kind}_night`) ?? 0,
    ] as [number, number, number],
    status: num(m, `${kind}_status`) ?? 0,
  })
  return { passenger: category('passenger'), freight: category('freight') }
}

function RailwayRows({ m }: { m: ContributorMetadata }) {
  const traffic = railTraffic(m)
  // A level crossing's horn carries its soundings in the passenger slots.
  const horn = text(m, 'rail_type') === 'horn'
  return (
    <>
      {speedLine(m)}
      {lineRow('Trains', (
        <HoverText title={railTrafficDescription(traffic, horn)}>{railTrafficLabel(traffic, horn)}</HoverText>
      ))}
    </>
  )
}

// A building or an open area has a footprint; the people at a venue, its bells or its call to prayer
// are a point, and their height is the source's own, not a building's.
function BuildingRows({ m }: { m: ContributorMetadata }) {
  const area = num(m, 'area_m2') ?? 0
  if (area <= 0) return null
  const height = num(m, 'height_m') ?? 0
  const floors = num(m, 'floors') ?? 0
  const cars = num(m, 'movements_per_day')
  const address = text(m, 'address')
  const facts = [
    ...(floors > 1 ? [`${floors} floors`] : []),
    ...(height > 0 ? [`${height.toFixed(0)} m high`] : []),
    `${fmtInt(area)} m²`,
  ]
  return (
    <>
      {lineRow(height > 0 ? 'Building' : 'Area', facts.join(' · '))}
      {cars != null && lineRow('Cars in and out', `${fmtCount(cars)}/day`)}
      {address && lineRow('Address', address)}
    </>
  )
}

function IndustrialRows({ m }: { m: ContributorMetadata }) {
  const area = num(m, 'area_m2') ?? 0
  const ratedKw = num(m, 'rated_power_kw')
  return (
    <>
      {area > 0 && lineRow('Area', `${fmtInt(area)} m²`)}
      {ratedKw != null && lineRow('Rated power', ratedKw >= 1000 ? `${(ratedKw / 1000).toFixed(1)} MW` : `${fmtInt(ratedKw)} kW`)}
    </>
  )
}

/** A ship cell's vessel-hours a month by class, in the order of the builder's display. */
const SHIP_CLASSES = ['large_ships', 'work_boats', 'leisure_craft'] as const
const HOURS_PER_MONTH = (365.25 * 24) / 12

function ShipRows({ m }: { m: ContributorMetadata }) {
  const hours = Array.isArray(m.hours_per_month) ? m.hours_per_month.map(Number) : []
  // A month's vessel-hours over the hours of a month: how many vessels are in the cell at a time.
  const atATime = SHIP_CLASSES.map((_, k) => (hours[k] ?? 0) / HOURS_PER_MONTH)
  const cell = txtTable([
    ...SHIP_CLASSES.map((name, k) => [subtypeLabel('ship', name), fmtCount(atATime[k])] as [string, string]),
    '',
    `On average in this ${((num(m, 'area_m2') ?? 0) / 1e6).toFixed(1)} km² cell, from the`,
    'vessel-hours of a month in AIS positions',
  ], 16, 8)
  return lineRow('Vessels at a time', <HoverText title={cell}>{fmtCount(atATime.reduce((a, b) => a + b, 0))}</HoverText>)
}

/** An airport's ground operations: its taxiing and rolls come from its flights. */
function AirportRows({ m }: { m: ContributorMetadata }) {
  const perDay = (key: string) => `${fmtCount(num(m, key) ?? 0)}/day`
  const airport = text(m, 'airport')
  return (
    <>
      {airport && lineRow('Airport', airport)}
      {lineRow('Arrivals', perDay('arrivals_per_day'))}
      {lineRow('Departures', perDay('departures_per_day'))}
      {(num(m, 'ground_vehicles_per_day') ?? 0) > 0 && lineRow('Ground vehicles', perDay('ground_vehicles_per_day'))}
    </>
  )
}

// A layer without lines of its own: its fields by name, but for the name and the subtype that the
// row and its class line show.
function FieldRows({ m }: { m: ContributorMetadata }) {
  const fields = Object.entries(m).filter(([name, value]) =>
    name !== 'name' && name !== 'subtype' && value !== null && value !== '' && value !== false)
  return <>{fields.map(([name, value]) => <div key={name}>{lineRow(words(name), fieldText(value))}</div>)}</>
}

const LAYER_ROWS: Record<string, (props: { m: ContributorMetadata }) => ReactNode> = {
  road: RoadRows,
  railway: RailwayRows,
  building: BuildingRows,
  industrial: IndustrialRows,
  ship: ShipRows,
  aircraft: AirportRows,
}

/** When a source's stored sound power holds: a venue's people at their busiest, an event while it
 *  sounds, every other source by day. */
function soundPowerWhen(m: ContributorMetadata): string {
  const type = text(m, 'building_type')
  if (type.startsWith('people_')) return 'at its busiest'
  return type === 'church_bells' || type === 'call_to_prayer' ? 'while sounding' : 'by day'
}

/** The layer's lines for a contributor, and its sound power where the layer stores one. */
export function MetadataRows({ c }: { c: Contributor }) {
  const m = c.metadata
  if (!m) return null
  const Rows = LAYER_ROWS[c.source_type] ?? FieldRows
  const soundPower = LAYER_ROWS[c.source_type] ? num(m, 'sound_power_dba') : null
  return (
    <>
      <Rows m={m} />
      {soundPower != null && lineRow(
        <HoverText title="How much noise the source itself makes, all of it (A-weighted sound power)">Sound power</HoverText>,
        `${soundPower.toFixed(1)} dB(A) ${soundPowerWhen(m)}`,
      )}
    </>
  )
}
