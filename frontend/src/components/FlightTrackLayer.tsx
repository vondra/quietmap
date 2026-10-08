// The highlighted source on the map, as dev1 draws it: a white line on a black casing for a loudest
// flight's track near the point or a contributor's lines, legible over every basemap and heatmap
// colour; a point source is a white dot ringed in black. The dot layer reads points only (a circle
// layer would put a dot on every vertex of the lines too).
import type { FilterSpecification } from 'maplibre-gl'
import { Layer, Source } from 'react-map-gl/maplibre'
import { highlightGeoJson } from './noise/top-flights'

// Round ends join the pieces of a bending track without notches.
const LINE_LAYOUT = { 'line-cap': 'round', 'line-join': 'round' } as const
// Multi-part geometries read as their part type in some MapLibre versions and not in others.
const LINES_ONLY: FilterSpecification = ['in', ['geometry-type'], ['literal', ['LineString', 'MultiLineString']]]
const POINTS_ONLY: FilterSpecification = ['in', ['geometry-type'], ['literal', ['Point', 'MultiPoint']]]

export default function FlightTrackLayer({ track }: { track: number[][][] | null }) {
  if (!track) return null
  return (
    <Source id="flight-track" type="geojson" data={highlightGeoJson(track)}>
      <Layer id="flight-track-casing" type="line" filter={LINES_ONLY} layout={LINE_LAYOUT} paint={{ 'line-color': '#000000', 'line-width': 5 }} />
      <Layer id="flight-track-line" type="line" filter={LINES_ONLY} layout={LINE_LAYOUT} paint={{ 'line-color': '#ffffff', 'line-width': 2 }} />
      <Layer
        id="flight-track-point"
        type="circle"
        filter={POINTS_ONLY}
        paint={{ 'circle-color': '#ffffff', 'circle-radius': 4, 'circle-stroke-color': '#000000', 'circle-stroke-width': 2 }}
      />
    </Source>
  )
}
