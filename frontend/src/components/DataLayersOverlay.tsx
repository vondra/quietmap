// The data layers switched on, drawn on the map from the served release over the heatmap and under
// the basemap's labels; enlarged, their cells stay hard-edged, and a new zoom replaces the last at
// once.
import { useEffect, useState } from 'react'
import { Layer, Source, useMap } from 'react-map-gl/maplibre'
import { DATA_LAYERS, dataLayerStyleId, type DataLayerId } from '../lib/data-layers'
import { labelAnchorId } from '../utils/label-layers'

export default function DataLayersOverlay({ layers }: { layers: DataLayerId[] }) {
  const { current: mapRef } = useMap()
  // Every style change redraws this, so each layer's place follows what the map holds now.
  const [, setStyleVersion] = useState(0)
  useEffect(() => {
    if (!mapRef) return
    const map = mapRef.getMap()
    const sync = () => setStyleVersion(version => version + 1)
    map.on('styledata', sync)
    return () => {
      map.off('styledata', sync)
    }
  }, [mapRef])
  const map = mapRef?.getMap()
  const style = map?.getStyle()?.layers
  const on = DATA_LAYERS.filter(layer => layers.includes(layer.id))
  // Each beneath the nearest one above it already on the map, else beneath the labels: created in
  // the panel's order they stack in it, and one switched on later, or re-added after a basemap
  // switch in any order, is moved into it (a layer moves when its beforeId changes).
  const beneath = (at: number) =>
    on.slice(at + 1).map(layer => dataLayerStyleId(layer.id)).find(id => map?.getLayer(id))
    ?? labelAnchorId(style)
  return on.map((layer, at) => (
    <Source
      key={layer.id}
      id={dataLayerStyleId(layer.id)}
      type="raster"
      tiles={[`/api/raster/${layer.id}/{z}/{x}/{y}.png`]}
      tileSize={256}
      minzoom={layer.minzoom}
      maxzoom={16}
    >
      <Layer
        id={dataLayerStyleId(layer.id)}
        type="raster"
        paint={{ 'raster-opacity': layer.opacity, 'raster-resampling': 'nearest', 'raster-fade-duration': 0 }}
        beforeId={beneath(at)}
      />
    </Source>
  ))
}
