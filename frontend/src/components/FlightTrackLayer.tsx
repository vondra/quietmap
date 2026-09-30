// The highlighted loudest flight's track on the map: the pieces the popup computed near the point, in
// the blue of the flight's link in the table (sky-700), on a white casing that keeps it legible over
// every basemap and heatmap colour.
import { Layer, Source } from 'react-map-gl/maplibre'
import type { TrackPiece } from '../types/noise'
import { flightTrackGeoJson } from './noise/top-flights'

// Round ends join the pieces of a bending track without notches.
const LINE_LAYOUT = { 'line-cap': 'round', 'line-join': 'round' } as const

export default function FlightTrackLayer({ track }: { track: TrackPiece[] | null }) {
  if (!track) return null
  return (
    <Source id="flight-track" type="geojson" data={flightTrackGeoJson(track)}>
      <Layer id="flight-track-casing" type="line" layout={LINE_LAYOUT} paint={{ 'line-color': '#ffffff', 'line-width': 6 }} />
      <Layer id="flight-track-line" type="line" layout={LINE_LAYOUT} paint={{ 'line-color': '#0069a8', 'line-width': 3 }} />
    </Source>
  )
}
