/** Parse admitted RWS INWEVA 2024 weekdag section intensities. */

import { withholdsCountLine } from './count-holdout.js'
import type { RoadTimeProfileEntry } from './roads-arrow.js'
import { roadObservation, type RoadObservation } from './road-observation.js'
import type { RoadLoaderArguments } from './road-loader-cli.js'
import { readPinnedRoadSource } from './pinned-road-source.js'

const SOURCE_PATH = 'nl/inweva-2024-weekdagen-ks.json'
const SOURCE_SHA256 = '7321b971e35d264e647c4cd43be3d21882b3ba9bbe994dea5db44950103a81b1'
const NETHERLANDS_BBOX = [50.7, 3.2, 53.7, 7.3] as const

/** Baansoorten with general motor-vehicle traffic: main carriageways, ramps, interchange
 *  connectors and parallel distributor roads. BUS/PST/WIS/TRB/GRB carry no through traffic
 *  (bus lanes, rest areas, tidal lanes, unknown codes) and are never admitted. */
const ADMITTED_BAANSOORT = new Set(['HR', 'OPR', 'AFR', 'VBR', 'VBD', 'VBS', 'VBI', 'VBW', 'VBK', 'NRB'])
const RAMP_BAANSOORT = new Set(['OPR', 'AFR', 'VBR', 'VBD', 'VBS', 'VBI', 'VBW', 'VBK'])

export interface DutchInwevaObservation extends RoadObservation {
  /** Road numbers (A2, N57) accepted at match time; ramps match by line alone. */
  refs: ReadonlySet<string>
  /** One carriageway line, or both twins of a paired two-way observation. */
  lines: ReadonlyArray<readonly (readonly [number, number])[]>
  rank: number
  isRamp: boolean
  light: number
  medium: number
  heavy: number
  moto: number
  /** Observed class timing; absent where RWS published no usable periods (the class default applies). */
  timeProfile?: DutchInwevaTimeProfile
}

/** Class-specific day/evening/night shares from the section's published period
 *  volumes, ready for the `roads_time_profiles` dictionary. Motorcycles follow
 *  light: loops cannot see them, so the 1 % moto share is imputed from l1. */
export interface DutchInwevaTimeProfile {
  shares: RoadTimeProfileEntry['profile']
  status: string
}

export interface DutchInwevaSource {
  observations: DutchInwevaObservation[]
  sourceRows: number
  /** Reciprocal opposite-direction sections counted as one two-way observation. */
  pairedSections: number
  lonelySections: number
  /** Sections whose value RWS derived away from a measurement (afst_mtw > 0). */
  derivedSectionsSkipped: number
  missingValuesSkipped: number
  unsupportedBaansoortSkipped: number
  /** Lonely N-road main carriageways: a directional count must not stamp a two-way row. */
  lonelyNationalRoadSkipped: number
  missingRefSkipped: number
  invalidGeometrySkipped: number
}

type UnknownRecord = Record<string, unknown>
const isRecord = (value: unknown): value is UnknownRecord =>
  typeof value === 'object' && value !== null && !Array.isArray(value)

const isCount = (value: unknown): value is number =>
  typeof value === 'number' && Number.isSafeInteger(value) && value >= 0

function lineCoordinates(value: unknown): (readonly [number, number])[] | null {
  if (!isRecord(value) || value.type !== 'LineString' || !Array.isArray(value.coordinates)) return null
  const result: (readonly [number, number])[] = []
  for (const coordinate of value.coordinates) {
    if (
      !Array.isArray(coordinate) ||
      typeof coordinate[0] !== 'number' ||
      typeof coordinate[1] !== 'number' ||
      !Number.isFinite(coordinate[0]) ||
      !Number.isFinite(coordinate[1]) ||
      coordinate[0] < NETHERLANDS_BBOX[1] ||
      coordinate[0] > NETHERLANDS_BBOX[3] ||
      coordinate[1] < NETHERLANDS_BBOX[0] ||
      coordinate[1] > NETHERLANDS_BBOX[2]
    )
      return null
    result.push([coordinate[0], coordinate[1]])
  }
  return result.length >= 2 ? result : null
}

interface InwevaSection {
  id: string
  twin: string | null
  baansoort: string
  refs: ReadonlySet<string>
  rank: number
  coordinates: (readonly [number, number])[]
  l1: number
  l2: number
  l3: number
  /** Published dag/avond/nacht volumes per loop class; null where RWS published none. */
  periods: readonly [
    readonly [number, number, number] | null,
    readonly [number, number, number] | null,
    readonly [number, number, number] | null,
  ]
  /** Raw RWS valid-day flags, kept for the profile status audit. */
  quality: string
}

function sectionRank(ref: string): number | null {
  if (ref.startsWith('A')) return 0
  if (ref.startsWith('N')) return 1
  return null
}

// RWS INWEVA periods are the engine's local periods: dag 07–19, avond 19–23,
// nacht 23–07 (RWS INWEVA layer field aliases "Vrachtpercentage dag (7 - 19)"
// and "Motorvoertuigen avond (19 - 23)"). dag+avond+nacht reproduce the etmaal
// within one vehicle on the pinned file, so these fields are period volumes.
function periodTriple(
  properties: UnknownRecord | null,
  prefix: 'l1' | 'l2' | 'l3',
): readonly [number, number, number] | null {
  const day = properties?.[`${prefix}_d_wk`],
    evening = properties?.[`${prefix}_a_wk`],
    night = properties?.[`${prefix}_n_wk`]
  return isCount(day) && isCount(evening) && isCount(night) ? [day, evening, night] : null
}

function qualityFlags(properties: UnknownRecord | null): string {
  const flag = (value: unknown): string => (typeof value === 'string' && value ? value : 'unknown')
  return `kwal_al=${flag(properties?.kwal_al)} kwal_vc=${flag(properties?.kwal_vc)}`
}

const PERIOD_CLASS_NAMES = ['light', 'medium', 'heavy'] as const

/** Class-specific period shares of one lonely or twin-paired observation: twin
 *  volumes sum per class and period first (a both-directions total), a class
 *  without usable periods keeps the class default, motorcycles follow light.
 *  Undefined where no class has usable periods. */
function sectionTimeProfile(sections: readonly InwevaSection[]): DutchInwevaTimeProfile | undefined {
  const shares: RoadTimeProfileEntry['profile'] = {}
  const notes: string[] = []
  PERIOD_CLASS_NAMES.forEach((name, index) => {
    const triples = sections.map(section => section.periods[index])
    const etmaal = sections.reduce((sum, section) => sum + [section.l1, section.l2, section.l3][index], 0)
    if (triples.some(triple => triple === null)) {
      notes.push(`${name} periods unpublished`)
      return
    }
    const day = triples.reduce((sum, triple) => sum + triple![0], 0)
    const evening = triples.reduce((sum, triple) => sum + triple![1], 0)
    const night = triples.reduce((sum, triple) => sum + triple![2], 0)
    if (day + evening + night > 0 && etmaal > 0) {
      const total = day + evening + night
      shares[name] = [day / total, evening / total, night / total]
    } else if (day + evening + night === 0 && etmaal === 0) {
      // Consistent zero: no traffic, nothing to time.
    } else {
      notes.push(`${name} periods inconsistent with etmaal`)
    }
  })
  if (Object.keys(shares).length === 0) return undefined
  if (shares.light) shares.moto = [...shares.light]
  const scope = sections.length > 1 ? 'twin sections, both-directions volumes summed' : 'single section'
  return {
    shares,
    status:
      `RWS INWEVA 2024 weekdag-gemiddelde (${scope}); class shares from published l1/l2/l3 ` +
      `dag/avond/nacht volumes, motorcycles follow light; per-section valid-day coverage ` +
      `unpublished (${sections.map(section => section.quality).join(' + ')})` +
      (notes.length ? `; ${notes.join('; ')}` : ''),
  }
}

function splitDutchTraffic(
  l1: number,
  l2: number,
  l3: number,
): { light: number; medium: number; heavy: number; moto: number } {
  // RWS loop length classes map onto light/medium/heavy; loops cannot see motorcycles,
  // so 1 % of the total rides as moto exactly as on the Danish and Finnish censuses.
  // The share comes out of l1 and can never exceed it (a tiny l1 would otherwise
  // drive light negative and abort the square).
  const moto = Math.min(Math.round((l1 + l2 + l3) * 0.01), l1)
  return { light: l1 - moto, medium: l2, heavy: l3, moto }
}

export function parseDutchInwevaSource(raw: string): DutchInwevaSource {
  const parsed = JSON.parse(raw) as unknown
  if (!isRecord(parsed) || !Array.isArray(parsed.features) || parsed.features.length === 0) {
    throw new Error('Dutch INWEVA source must be a non-empty GeoJSON FeatureCollection')
  }
  const result: DutchInwevaSource = {
    observations: [],
    sourceRows: parsed.features.length,
    pairedSections: 0,
    lonelySections: 0,
    derivedSectionsSkipped: 0,
    missingValuesSkipped: 0,
    unsupportedBaansoortSkipped: 0,
    lonelyNationalRoadSkipped: 0,
    missingRefSkipped: 0,
    invalidGeometrySkipped: 0,
  }
  const sections = new Map<string, InwevaSection>()
  for (const feature of parsed.features) {
    const properties = isRecord(feature) && isRecord(feature.properties) ? feature.properties : null
    const id = properties?.vbn_id
    const baansoort = properties?.bnsubsrt_b
    const twin = properties?.vbn_id_tgn ?? null
    if (
      typeof id !== 'string' ||
      !id ||
      typeof baansoort !== 'string' ||
      !(twin === null || typeof twin === 'string') ||
      sections.has(id)
    ) {
      throw new Error('Dutch INWEVA feature has invalid section identity')
    }
    if (!ADMITTED_BAANSOORT.has(baansoort)) {
      result.unsupportedBaansoortSkipped++
      continue
    }
    const coordinates = isRecord(feature) ? lineCoordinates(feature.geometry) : null
    if (coordinates === null) {
      result.invalidGeometrySkipped++
      continue
    }
    // afst_mtw is the distance to the measurement: only sections measured at their own
    // loops count as measured; adjacent derived sections keep the prior or a continuity fill.
    if (properties?.afst_mtw !== 0) {
      result.derivedSectionsSkipped++
      continue
    }
    const l1 = properties?.l1_e_wk,
      l2 = properties?.l2_e_wk,
      l3 = properties?.l3_e_wk
    if (!isCount(l1) || !isCount(l2) || !isCount(l3) || l1 + l2 + l3 === 0) {
      result.missingValuesSkipped++
      continue
    }

    const refs = new Set<string>()
    for (const road of [properties?.wegnrhmp_b, properties?.wegnrhmp_e]) {
      if (typeof road === 'string' && road.trim()) refs.add(road.trim().toUpperCase().replace(/\s+/g, ''))
    }
    if (refs.size === 0) {
      result.missingRefSkipped++
      continue
    }
    const rank = sectionRank([...refs][0])
    if (rank === null) {
      result.missingRefSkipped++
      continue
    }
    sections.set(id, {
      id,
      twin: twin && twin !== id ? twin : null,
      baansoort,
      refs,
      rank,
      coordinates,
      l1,
      l2,
      l3,
      periods: [periodTriple(properties, 'l1'), periodTriple(properties, 'l2'), periodTriple(properties, 'l3')],
      quality: qualityFlags(properties),
    })
  }
  const paired = new Set<string>()
  const admit = (
    section: InwevaSection,
    basis: 'directional' | 'both-directions',
    observationId: string,
    lines: ReadonlyArray<readonly (readonly [number, number])[]>,
    l1: number,
    l2: number,
    l3: number,
  ): void => {
    if (lines.some(withholdsCountLine)) return
    result.observations.push({
      ...roadObservation(observationId, basis),
      refs: section.refs,
      lines,
      rank: section.rank,
      isRamp: RAMP_BAANSOORT.has(section.baansoort),
      ...splitDutchTraffic(l1, l2, l3),
      timeProfile: sectionTimeProfile([section]),
    })
  }
  for (const section of sections.values()) {
    if (paired.has(section.id)) continue
    const twin = section.twin !== null ? (sections.get(section.twin) ?? null) : null
    // A reciprocal twin pair of main carriageways is one two-way observation: either
    // carriageway matches the pair and roads-finalize shares the total through the shared
    // observation identity. Numbering transitions (A7/N7) take the wider rank.
    if (twin !== null && twin.twin === section.id && section.baansoort === 'HR' && twin.baansoort === 'HR') {
      paired.add(section.id)
      paired.add(twin.id)
      result.pairedSections += 2
      const refs = new Set([...section.refs, ...twin.refs])
      const lines = [section.coordinates, twin.coordinates] as const
      if (lines.some(withholdsCountLine)) continue
      result.observations.push({
        ...roadObservation(`inweva2024:${[section.id, twin.id].sort().join('-')}`, 'both-directions'),
        refs,
        lines,
        rank: Math.max(section.rank, twin.rank),
        isRamp: false,
        ...splitDutchTraffic(section.l1 + twin.l1, section.l2 + twin.l2, section.l3 + twin.l3),
        timeProfile: sectionTimeProfile([section, twin]),
      })
      continue
    }
    // Every INWEVA section carries one direction's flow (lonely A-road medians sit at half
    // their twin-pair sums). A lonely N-road section would halve a two-way OSM row, so only
    // lonely A-road, ramp and connector sections stamp their own carriageway directionally.
    if (section.baansoort === 'HR' && section.rank === 1) {
      result.lonelyNationalRoadSkipped++
      continue
    }
    paired.add(section.id)
    result.lonelySections++
    admit(section, 'directional', `inweva2024:${section.id}`, [section.coordinates], section.l1, section.l2, section.l3)
  }
  if (result.observations.length === 0) throw new Error('Dutch INWEVA source has no usable measurements')
  return result
}

export function loadDutchInwevaSource(options: RoadLoaderArguments): DutchInwevaSource {
  return parseDutchInwevaSource(readPinnedRoadSource(options, SOURCE_PATH, SOURCE_SHA256).toString('utf8'))
}
