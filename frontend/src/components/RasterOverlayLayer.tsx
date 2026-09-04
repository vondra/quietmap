import { useEffect, useState } from 'react'
import { Source, Layer, useMap } from 'react-map-gl/maplibre'

interface RasterOverlayLayerProps {
  visibleLayers: Record<string, boolean>
}

/// `url` overrides the default `/api/raster/{id}/{z}/{x}/{y}.png` when
/// non-empty. Noise levels come from the point compute popup, not a raster.
const LAYERS = [
  { id: 'dem', minzoom: 6, url: '' },
  // Overlay key `building-height` (both share the flat rasterOverlays dict);
  // the tile path stays /api/raster/building/.
  { id: 'building-height', minzoom: 10, url: '/api/raster/building/{z}/{x}/{y}.png' },
  { id: 'forest', minzoom: 8, url: '' },
  { id: 'barriers', minzoom: 12, url: '' },
] as const

export default function RasterOverlayLayer({ visibleLayers }: RasterOverlayLayerProps) {
  const { current: mapRef } = useMap()
  const [styleState, setStyleState] = useState({
    epoch: 0,
    ready: false,
    ceilingReady: false,
  })

  useEffect(() => {
    if (!mapRef) return

    const map = mapRef.getMap()

    const sync = () => {
      setStyleState(prev => {
        const nextReady = !!map.isStyleLoaded()
        const nextCeilingReady = !!map.getLayer('_deck-ceiling')
        if (prev.ready === nextReady && prev.ceilingReady === nextCeilingReady) {
          return prev
        }
        return {
          epoch: prev.epoch + 1,
          ready: nextReady,
          ceilingReady: nextCeilingReady,
        }
      })
    }

    sync()
    map.on('style.load', sync)
    map.on('idle', sync)

    return () => {
      map.off('style.load', sync)
      map.off('idle', sync)
    }
  }, [mapRef])

  if (!styleState.ready) return null

  const beforeId = styleState.ceilingReady ? '_deck-ceiling' : undefined

  return (
    <>
      {LAYERS.map(({ id, minzoom, url }) =>
        visibleLayers[id] ? (
          <Source
            key={`${id}-${styleState.epoch}`}
            id={`raster-${id}`}
            type="raster"
            tiles={[url || `/api/raster/${id}/{z}/{x}/{y}.png`]}
            tileSize={256}
            minzoom={minzoom}
            maxzoom={16}
          >
            <Layer
              id={`raster-${id}-layer`}
              type="raster"
              paint={{ 'raster-opacity': 0.7 }}
              {...(beforeId ? { beforeId } : {})}
            />
          </Source>
        ) : null,
      )}
    </>
  )
}
