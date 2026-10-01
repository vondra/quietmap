// A contributor's display fields as rows: speed, traffic and surface of a road, trains of a
// railway, the site of an industry, building or ship cell. A layer without rows of its own lists
// its fields by name, so a newly added layer shows up before it gets its own wording.
import type { ReactNode } from 'react'
import type { Contributor, ContributorMetadata } from '../../../types/noise'
import { fmt, fmtCompact, fmtInt, txtTable, type TableRow } from '../../../utils/formatters'
import { MetricLabel, DataPoint } from '../noise-tooltips'
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
  const speed = num(m, 'speed_kmh')
  const posted = num(m, 'speed_posted_kmh')
  const speedSource = text(m, 'speed_source')
  // Derestricted (e.g. a German Autobahn) has no posted number; a missing number otherwise means
  // no posted limit and a default speed.
  const postedText = speedSource === 'derestricted' ? 'no limit' : posted != null && posted > 0 ? `${posted} km/h` : '— (none)'
  const speedText = txtTable([
    ['Source', words(speedSource)],
    ['Posted maxspeed', postedText],
    ['Road class', words(text(m, 'road_class'))],
    { sep: true },
    ['Used', speed != null ? `${speed.toFixed(0)} km/h` : '—'],
  ], 18, 12)
  const trafficText = txtTable([
    ...(wholeRoad
      ? [['Whole road', `${fmtInt(crossSection)}/day`] as [string, string], 'both directions', '']
      : total === 0
        ? ['This carriageway carries no traffic.', '']
        : ['Only this direction is known.', '']),
    'This carriageway:',
    ...([['Light', light, 1], ['Medium', medium, 2], ['Heavy', heavy, 4], ['Moto', moto, 8]] as const)
      .map(([label, value, bit]) =>
        [label, roadCategoryEstimated(estimated, bit) ? `${fmtInt(value)} (est.)` : fmtInt(value)] as [string, string],
      ),
    { sep: true },
    ['Total', `${fmtInt(total)}/day`],
    '',
    'Counts are prepared per vehicle class:',
    'a counted value is an observation, an',
    '"(est.)" value is an estimate or class',
    'prior from the build.',
  ] as TableRow[], 18, 12)
  const lanes = num(m, 'lanes')
  const surfaceText = txtTable([
    ['Type', text(m, 'surface')],
    ['Rolling correction', `${fmt(num(m, 'surface_corr_db') ?? 0)} dB`],
    ['Lanes', lanes != null && lanes > 0 ? String(lanes) : 'unknown'],
    ['Oneway', m.oneway === true ? 'yes' : 'no'],
    ...(m.bridge === true ? [['Bridge', 'yes'] as [string, string]] : []),
  ], 18, 12)
  return (
    <>
      {speed != null && lineRow(
        <MetricLabel term="speed" />,
        <DataPoint title="Speed used in CNOSSOS emission" text={speedText}>
          {speed.toFixed(0)} km/h
        </DataPoint>,
      )}
      {lineRow(
        <MetricLabel term="aadt">Traffic</MetricLabel>,
        <DataPoint title={wholeRoad ? 'Daily traffic on the whole road, both directions' : 'Daily traffic in this direction'} text={trafficText}>
          {wholeRoad ? `${fmtCompact(crossSection)}/day` : `${fmtCompact(total)}/day · one direction`}
        </DataPoint>,
      )}
      {lineRow(
        <MetricLabel term="surface">Surface</MetricLabel>,
        <DataPoint title="CNOSSOS surface correction" text={surfaceText}>
          {text(m, 'surface')}
        </DataPoint>,
      )}
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
  const speed = num(m, 'speed_kmh')
  const traffic = railTraffic(m)
  const speedText = txtTable([
    ['Source', words(text(m, 'speed_source'))],
    ['Rail type', words(text(m, 'rail_type'))],
    ['Usage', words(text(m, 'usage'))],
    ...(m.bridge === true ? [['Bridge', 'yes'] as [string, string]] : []),
    { sep: true },
    ['Used', speed != null ? `${speed.toFixed(0)} km/h` : '—'],
  ], 18, 14)
  return (
    <>
      {speed != null && lineRow(
        <MetricLabel term="speed" />,
        <DataPoint title="Speed used in CNOSSOS emission" text={speedText}>
          {speed.toFixed(0)} km/h
        </DataPoint>,
      )}
      {lineRow(
        <MetricLabel term="trains">Trains/day</MetricLabel>,
        <DataPoint title="Expected passages, with passenger and freight evidence shown separately." text={railTrafficDescription(traffic, text(m, 'rail_type') === 'horn')}>
          {railTrafficLabel(traffic)}
        </DataPoint>,
      )}
    </>
  )
}

/** Building-layer types that are open-air activity areas, not buildings (no floors, no height). */
const ACTIVITY_AREAS = new Set([
  'padel_court', 'tennis_court', 'ball_court', 'playground', 'swimming_pool', 'outdoor_seating',
  'stadium', 'sports_pitch', 'car_park', 'street_parking', 'motorsport', 'motorsport_circuit',
  'motorsport_motocross', 'motorsport_kart', 'motorsport_speedway', 'motorsport_trial', 'shooting',
  'shooting_rifle', 'shooting_pistol', 'shooting_shotgun',
])

// Only what the title does not already say: the type is the title (or the class line above), so the
// line lists the floors, mapped height and footprint the data holds, and no line without them.
function BuildingRows({ m }: { m: ContributorMetadata }) {
  const type = text(m, 'building_type')
  const height = num(m, 'height_m') ?? 0
  const floors = num(m, 'floors') ?? 0
  const area = num(m, 'area_m2') ?? 0
  const address = text(m, 'address')
  const activity = ACTIVITY_AREAS.has(type)
  const facts = [
    ...(!activity && floors > 1 ? [`${floors} floors`] : []),
    ...(!activity && height > 0 ? [`${height.toFixed(0)} m high`] : []),
    ...(area > 0 ? [`${fmtInt(area)} m²`] : []),
  ]
  if (!facts.length && !address) return null
  const detail = txtTable([...facts, ...(address ? ['', `Address: ${address}`] : [])], 14, 20)
  return lineRow(
    activity ? 'Area' : 'Building',
    <DataPoint title={activity ? 'Activity area' : 'Building'} text={detail}>
      {facts.join(' · ') || address}
    </DataPoint>,
  )
}

function IndustrialRows({ m }: { m: ContributorMetadata }) {
  const area = num(m, 'area_m2') ?? 0
  const gridPoints = num(m, 'grid_points') ?? 0
  const hubHeight = num(m, 'hub_height_m')
  const ratedPower = num(m, 'rated_power_kw')
  const nace = text(m, 'nace')
  const siteText = txtTable([
    ['Type', words(text(m, 'source_type'))],
    ...(area > 0 ? [['Area', `${fmtInt(area)} m²`] as [string, string]] : []),
    ...(nace ? [['NACE', nace] as [string, string]] : []),
    ...(gridPoints > 0 ? [['Grid points', String(gridPoints)] as [string, string]] : []),
    ...(hubHeight != null ? [['Hub height', `${hubHeight.toFixed(0)} m`] as [string, string]] : []),
    ...(ratedPower != null ? [['Rated power', `${fmtInt(ratedPower)} kW`] as [string, string]] : []),
    ...(gridPoints > 1
      ? ['', 'Large sites are split into grid points;', 'each carries its area share', 'of the total sound power.']
      : []),
  ], 16, 16)
  // The site type is the title (or the class line above): the line shows its area, if known.
  if (area <= 0) return null
  return lineRow(
    'Site',
    <DataPoint title="Industrial site metadata" text={siteText}>
      {`${fmtInt(area)} m²`}
    </DataPoint>,
  )
}

function ShipRows({ m }: { m: ContributorMetadata }) {
  const hours = Array.isArray(m.hours_per_month) ? m.hours_per_month.map(Number) : []
  const [large = 0, work = 0, leisure = 0] = hours
  const cellText = txtTable([
    ['Loudest', words(text(m, 'source_type'))],
    ['Cell', `${((num(m, 'area_m2') ?? 0) / 1e6).toFixed(2)} km²`],
    ['Large ships', `${large.toFixed(1)} h/month`],
    ['Work boats', `${work.toFixed(1)} h/month`],
    ['Leisure craft', `${leisure.toFixed(1)} h/month`],
    '', 'Mean vessel-hours per month in this', 'water cell, from AIS positions;', 'ships radiate around the clock.',
  ], 16, 16)
  return lineRow(
    'Ships',
    <DataPoint title="Ship traffic cell" text={cellText}>
      {`${(large + work + leisure).toFixed(0)} h/month`}
    </DataPoint>,
  )
}

// A layer without rows of its own: its fields by name, but for the name and the subtype that the
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
}

/** The layer's rows for a contributor, and its sound power where the layer stores one. */
export function MetadataRows({ c }: { c: Contributor }) {
  const m = c.metadata
  if (!m) return null
  const Rows = LAYER_ROWS[c.source_type] ?? FieldRows
  const soundPower = LAYER_ROWS[c.source_type] ? num(m, 'sound_power_dba') : null
  return (
    <>
      <Rows m={m} />
      {soundPower != null && lineRow(
        <MetricLabel term="emission" />,
        <DataPoint title="Sound power Lw of the source by day (A-weighted)" text="Summed over every point of the source.">
          {soundPower.toFixed(1)} dB(A)
        </DataPoint>,
      )}
    </>
  )
}
