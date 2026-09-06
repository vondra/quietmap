/** Admit retained CZPTT sequences and resolve the original ordered station-name join. */

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import type { RailStationPairCount } from './rail-graph.js'

export interface CzpttTrainStop { code: string; name: string }
export interface CzpttTrainSequence { stops: CzpttTrainStop[]; isFreight: boolean }
interface StationGPS { name: string; lat: number; lon: number }

function normName(s: string): string {
  return s.toLowerCase()
    .replace(/[- ]/g, '')
    .replace(/pha\s*hl\.?n\.?/i, 'prahahlavn')
    .replace(/hl\.?n\.?/i, 'hlavní nádraží')
    .replace(/[áà]/g, 'a').replace(/[éè]/g, 'e').replace(/[íì]/g, 'i')
    .replace(/[óò]/g, 'o').replace(/[úùů]/g, 'u').replace(/ý/g, 'y')
    .replace(/č/g, 'c').replace(/ď/g, 'd').replace(/ň/g, 'n')
    .replace(/ř/g, 'r').replace(/š/g, 's').replace(/ť/g, 't').replace(/ž/g, 'z')
    .replace(/ě/g, 'e')
}

function buildCodeToGPS(
  sequences: readonly CzpttTrainSequence[],
  stationGPS: Map<string, StationGPS>,
): Map<string, { lat: number; lon: number }> {
  const codeToGPS = new Map<string, { lat: number; lon: number }>()
  const normOSM = new Map<string, StationGPS>()
  for (const [name, gps] of stationGPS) normOSM.set(normName(name), gps)

  for (const seq of sequences) {
    for (const { code, name } of seq.stops) {
      if (codeToGPS.has(code)) continue
      const osm = stationGPS.get(name) || normOSM.get(normName(name))
      if (osm) {
        codeToGPS.set(code, { lat: osm.lat, lon: osm.lon })
      }
    }
  }
  return codeToGPS
}

export function czpttSequencesToStationPairs(
  sequences: readonly CzpttTrainSequence[],
  codeToGPS: ReadonlyMap<string, { lat: number; lon: number }>,
): RailStationPairCount[] {
  const pairs: RailStationPairCount[] = []
  for (const seq of sequences) {
    const known: Array<{ code: string; lat: number; lon: number }> = []
    for (const stop of seq.stops) {
      const gps = codeToGPS.get(stop.code)
      if (!gps) continue // pseudo-location or OSM-untagged station — bridged by omission
      if (known.length > 0 && known[known.length - 1].code === stop.code) continue // adjacent duplicate after resolution
      known.push({ code: stop.code, lat: gps.lat, lon: gps.lon })
    }
    if (known.length < 2) continue
    for (let i = 0; i < known.length - 1; i++) {
      pairs.push({
        fromLat: known[i].lat, fromLon: known[i].lon,
        toLat: known[i + 1].lat, toLon: known[i + 1].lon,
        pax: seq.isFreight ? 0 : 1,
        frt: seq.isFreight ? 1 : 0,
      })
    }
  }
  return pairs
}

function object(value: unknown, label: string): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error(`${label} must be an object`)
  return value as Record<string, unknown>
}

/** Missing, empty or malformed source inputs fail before any prepared writes. */
export function readCzpttSource(sourceDirectory: string): {
  sequences: number; stations: number; resolvedCodes: number; passengerTrains: number;
  freightTrains: number; pairs: RailStationPairCount[];
} {
  const trainPath = resolve(sourceDirectory, 'czptt-train-sequences.json')
  const trains = object(JSON.parse(readFileSync(trainPath, 'utf8')), trainPath).sequences
  if (!Array.isArray(trains) || trains.length === 0) throw new Error(`${trainPath}: no train sequences`)
  const sequences: CzpttTrainSequence[] = trains.map((value, index) => {
    const train = object(value, `${trainPath}:${index}`)
    if (typeof train.isFreight !== 'boolean' || !Array.isArray(train.stops)) {
      throw new Error(`${trainPath}:${index}: invalid train classification or stops`)
    }
    const stops = train.stops.map((value, stopIndex) => {
      const stop = object(value, `${trainPath}:${index}:${stopIndex}`)
      if (typeof stop.code !== 'string' || !/^\d+$/.test(stop.code) ||
          typeof stop.name !== 'string' || stop.name.length === 0) {
        throw new Error(`${trainPath}:${index}:${stopIndex}: invalid stop`)
      }
      return { code: stop.code, name: stop.name }
    })
    return { stops, isFreight: train.isFreight }
  })
  const stationPath = resolve(sourceDirectory, 'osm-stations.json')
  const records = object(JSON.parse(readFileSync(stationPath, 'utf8')), stationPath)
  const stations = new Map<string, StationGPS>()
  for (const [name, value] of Object.entries(records)) {
    const point = object(value, `${stationPath}:${name}`)
    if (!name || typeof point.name !== 'string' || typeof point.lat !== 'number' ||
        typeof point.lon !== 'number' || !Number.isFinite(point.lat) || !Number.isFinite(point.lon) ||
        point.lat < -90 || point.lat > 90 || point.lon < -180 || point.lon > 180) {
      throw new Error(`${stationPath}:${name}: invalid station coordinates`)
    }
    stations.set(name, { name: point.name, lat: point.lat, lon: point.lon })
  }
  if (stations.size === 0) throw new Error(`${stationPath}: no station coordinates`)
  const codes = buildCodeToGPS(sequences, stations)
  const pairs = czpttSequencesToStationPairs(sequences, codes)
  if (pairs.length === 0) throw new Error('CZPTT inputs resolve no station pairs')
  const freightTrains = sequences.filter(sequence => sequence.isFreight).length
  return { sequences: sequences.length, stations: stations.size, resolvedCodes: codes.size,
    passengerTrains: sequences.length - freightTrains, freightTrains, pairs }
}
