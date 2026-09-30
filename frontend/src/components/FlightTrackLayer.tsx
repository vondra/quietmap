// The highlighted source on the map: a loudest flight's track (the pieces the popup computed near the
// point) or a contributor's loudest pieces, in the blue of the flight's link in the table (sky-700),
// on a white casing that keeps it legible over every basemap and heatmap colour; a point source is a
// dot.
import { Layer, Source } from 'react-map-gl/maplibre'
import { highlightGeoJson } from './noise/top-flights'

// Round ends join the pieces of a bending track without notches.
const LINE_LAYOUT = { 'line-cap': 'round', 'line-join': 'round' } as const

export default function FlightTrackLayer({ track }: { track: number[][][] | null }) {
  if (!track) return null
  return (
    <Source id="flight-track" type="geojson" data={highlightGeoJson(track)}>
      <Layer id="flight-track-casing" type="line" layout={LINE_LAYOUT} paint={{ 'line-color': '#ffffff', 'line-width': 6 }} />
      <Layer id="flight-track-line" type="line" layout={LINE_LAYOUT} paint={{ 'line-color': '#0069a8', 'line-width': 3 }} />
      <Layer
        id="flight-track-point"
        type="circle"
        paint={{ 'circle-color': '#0069a8', 'circle-radius': 6, 'circle-stroke-color': '#ffffff', 'circle-stroke-width': 2 }}
      />
    </Source>
  )
}
