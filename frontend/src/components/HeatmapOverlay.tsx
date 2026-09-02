import { useCallback, useEffect, useRef, useState } from 'react'
import { MapboxOverlay } from '@deck.gl/mapbox'
import { TileLayer } from '@deck.gl/geo-layers'
import { BitmapLayer, GeoJsonLayer } from '@deck.gl/layers'
import { useMap } from 'react-map-gl/maplibre'

import { TILE_PX } from '../lib/hm3-decoder'
import { PREVIEW_DELTA } from '../lib/hm3-compose'
import { lngLatToTileFloat } from '../lib/tile-math'
import { MIN_ZOOM, WORLD_EXTENT, buildKey, tileUrl, useTileBuild, type TileBuilds } from '../lib/tile-urls'
import { loadTileProgressively, fetchAncestor, type HeatTile } from '../lib/progressive-tile-loader'
import { compositeSig, baseRange, buildComposite, type Composite } from '../lib/tile-composite'

// The seven toggleable noise layers. All share the same HM3 format + palette
// (Lden), so the tile fetch/decode/energy-sum loop is layer-agnostic.
export const HEATMAP_LAYERS = [
  'road',
  'rail',
  'industrial',
  'building',
  'aircraft-ground',
  'aircraft-airborne',
  'aircraft-cruise',
] as const

export type HeatmapLayer = (typeof HEATMAP_LAYERS)[number]

// The overlay can also fetch the precomputed `total` (energy-sum of all seven).
// MapView passes `['total']` when every layer is on — the common case, one fetch.
export type HeatmapSource = HeatmapLayer | 'total'

interface Props {
  sources: readonly HeatmapSource[]
  highlightGeometry?: GeoJSON.Geometry | null
}

// Display zoom at/above which we stitch ONE composite image (no internal tile
// borders → no seam) instead of the per-tile TileLayer: one level past the
// published base zoom, so it starts exactly where deck runs out of native
// levels. deck already over-zooms the base tiles from ~half a zoom up
// (tileSize 512), so a faint seam exists below this too — but it only gets
// objectionable when magnified further, and only this deep does the viewport
// span few enough base tiles for the stitch to stay cheap (the
// MAX_COMPOSITE_TILES cap). Tuned by eye.
function overzoomFrom(build: TileBuilds): number {
  return build.zoom + 1
}
// Decoded-tile cache bound. Each cached tile holds a TILE_PX² RGBA ImageData
// (~1 MiB CPU heap) plus its GPU texture, so 192 ≈ 192 MiB decoded heap and
// comparable GPU memory; deck may transiently exceed it for visible/selected
// tiles. The previous 512 allowed ~½ GB on a long touring session (HiDPI
// fills 4× faster) — a mobile-Safari tab-eviction budget. 192 ≈ 4-5 desktop
// screenfuls, deep enough for the ancestor fallback ladder (deck never
// evicts a visible fallback tile).
const MAX_CACHE_TILES = 192


/**
 * Render the noise heatmap with TWO modes, switched by zoom:
 *
 *  - **Normal zoom (up to the published base zoom):** deck.gl `TileLayer` of
 *    per-tile `BitmapLayer`s. deck owns viewport tile selection, fetch, cache and
 *    parent-tile fallback — fast and native. At native resolution the per-tile seam is sub-pixel.
 *  - **Over-zoom (one level past it):** ONE stitched composite `BitmapLayer` over the few
 *    base-zoom tiles under the viewport. A single texture has no internal borders, so
 *    the seam that per-tile clamp-to-edge sampling shows when magnified is gone.
 *    Few tiles at this zoom, so the stitch is cheap.
 *
 * Each tile (and the composite) is fetched as static `.bin`, decoded +
 * energy-summed (multi-layer subsets) + palette-mapped to `ImageData` in the
 * browser — server stays a dumb static/CDN reader. Interleaved overlay + a
 * `beforeId` of the first label layer keeps city labels on top.
 */
export default function HeatmapOverlay({ sources, highlightGeometry }: Props): null {
  const { current: mapRef } = useMap()
  // Generation snapshot: a build flip re-renders and re-keys every layer; the
  // snapshot is passed explicitly into URL builders so no fetch closure ever
  // reads newer module state than the layer it feeds (no mixed generations).
  const build = useTileBuild()
  const [overlay, setOverlay] = useState<MapboxOverlay | null>(null)
  const labelAnchor = useRef<string | undefined>(undefined)
  // The stitched over-zoom composite. `sig` is the source + base-zoom-range key
  // it was built for, so a pan within the same tiles skips the rebuild.
  const composite = useRef<(Composite & { sig: string }) | null>(null)
  // Bumped before each build so a slow stitch can't overwrite a newer one.
  const buildSeq = useRef(0)
  const applyRef = useRef<() => void>(() => {})
  const pending = useRef(false)
  const mounted = useRef(true)
  useEffect(() => {
    mounted.current = true
    return () => { mounted.current = false }
  }, [])

  // Progressive-refine repaint: a tile whose late layers landed swapped its
  // image in place — bump the trigger and re-apply (coalesced per frame) so
  // deck re-runs renderSubLayers and uploads the new ImageData.
  const refineSeq = useRef(0)
  const refinePending = useRef(false)
  const onRefined = useCallback(() => {
    refineSeq.current++
    if (refinePending.current) return
    refinePending.current = true
    requestAnimationFrame(() => {
      refinePending.current = false
      if (mounted.current) applyRef.current()
    })
  }, [])
  // Live progressive tails + the loaded-tile registry of the CURRENT tile
  // layer. deck never aborts a tile it already resolved and never calls
  // onTileUnload when finalizing a whole layer — so a layer swap (build/source
  // re-key, composite mode, unmount) must cancel the tails here. `loaded`
  // mirrors deck's session cache (z/x/y keys) so the ancestor preview can
  // yield to deck's own better fallback (owner policy 2026-07-16: exact zoom >
  // z−1 > z−2 > … > coarse preview > blank).
  const tileTails = useRef<{
    key: string
    aborts: Set<() => void>
    loaded: Set<string>
    /** In-flight request token per tile key — a resolve registers in `loaded`
     *  only while ITS token is still current (deck can evict a pending tile
     *  without aborting it, and it holds its own wrapper promise, so promise
     *  identity cannot be used to detect that). */
    inFlight: Map<string, symbol>
  }>({ key: '', aborts: new Set(), loaded: new Set(), inFlight: new Map() })
  const dropTileTails = useCallback((nextKey: string) => {
    if (tileTails.current.key === nextKey) return
    for (const abort of tileTails.current.aborts) abort()
    tileTails.current.aborts.clear()
    tileTails.current.loaded.clear()
    tileTails.current.inFlight.clear()
    tileTails.current.key = nextKey
  }, [])
  useEffect(() => () => dropTileTails(''), [dropTileTails])
  // Retina/HiDPI: fetch one pyramid level finer so one data cell ≈ one device
  // pixel on the zooms where a finer level exists (≤ z11 viewports; z12 is the
  // data floor). Costs 4× tiles for DPR ≥ 1.5 screens — owner call 2026-07-16
  // ("to chci"). Re-render if the window moves to a screen with different DPR.
  const [dpr, setDpr] = useState(() => (typeof window === 'undefined' ? 1 : window.devicePixelRatio))
  useEffect(() => {
    const mq = window.matchMedia(`(resolution: ${dpr}dppx)`)
    const onChange = () => setDpr(window.devicePixelRatio)
    mq.addEventListener('change', onChange)
    return () => mq.removeEventListener('change', onChange)
  }, [dpr])
  // A DPR flip re-creates `apply` but nothing else calls it — apply NOW so the
  // re-keyed layer (offset is part of the id) fetches the finer/coarser level.
  useEffect(() => { applyRef.current() }, [dpr])

  // One interleaved MapboxOverlay (shares MapLibre's GL context).
  useEffect(() => {
    if (!mapRef) return
    const map = mapRef.getMap()
    const next = new MapboxOverlay({ interleaved: true, layers: [] })
    map.addControl(next)
    setOverlay(next)
    return () => {
      map.removeControl(next)
      setOverlay(null)
    }
  }, [mapRef])

  // Pick the mode by zoom and push the deck layers. Cheap — no fetch (the
  // composite is built in `update`); also handles label-anchor + highlight changes.
  const apply = useCallback(() => {
    if (!overlay || !mapRef) return
    const map = mapRef.getMap()
    const beforeId = labelAnchor.current
    const layers = []
    // No published build yet (manifest still resolving) → no tile layers; the
    // store notification re-renders us the moment it lands.
    if (sources.length > 0 && build !== null) {
      const c = composite.current
      const overzoom = map.getZoom() >= overzoomFrom(build)
      // Paint the stitched composite ONLY while its `sig` matches the live view.
      // After a layer toggle (or a pan/zoom that out-raced its rebuild) the cached
      // composite is for the wrong source/range — fall back to the per-tile
      // TileLayer (correct data, maybe a faint seam) rather than paint stale tiles,
      // until `update` rebuilds the matching composite.
      if (overzoom && c && c.sig === compositeSig(build, sources, baseRange(map.getBounds(), build))) {
        dropTileTails('') // the tile layer is gone — its tails must not linger
        layers.push(new BitmapLayer({
          id: 'hm3-composite',
          image: c.image,
          bounds: c.bounds,
          beforeId,
          textureParameters: { minFilter: 'linear', magFilter: 'linear' },
        }))
      } else {
        const zoomOffset = dpr >= 1.5 ? 1 : 0
        // Offset is part of the registry key AND the layer id: a DPR flip must
        // re-key the layer (deck won't recompute tile selection on an options
        // change alone) and start a fresh loaded/tails registry with it.
        dropTileTails(`${buildKey(build, sources)}-o${zoomOffset}`)
        layers.push(makeHeatmapTileLayer(
          build, sources, beforeId, refineSeq.current, onRefined,
          tileTails.current, zoomOffset,
        ))
      }
    } else {
      dropTileTails('') // every source off (or no build) removes the layer too
    }
    layers.push(...makeHighlightLayers(highlightGeometry))
    overlay.setProps({ layers })
  }, [overlay, mapRef, build, sources, highlightGeometry, onRefined, dropTileTails, dpr])
  applyRef.current = apply

  // Rebuild the over-zoom composite (only past the composite threshold, only when the base
  // range/sources changed) then re-apply. Coalesced to one run per frame. Below it this is
  // just a re-apply (deck drives the TileLayer itself).
  const update = useCallback(() => {
    if (pending.current) return
    pending.current = true
    requestAnimationFrame(async () => {
      pending.current = false
      const map = mapRef?.getMap()
      if (!map || !mounted.current) return
      if (build !== null && sources.length > 0 && map.getZoom() >= overzoomFrom(build)) {
        const range = baseRange(map.getBounds(), build)
        const sig = compositeSig(build, sources, range)
        if (composite.current?.sig !== sig) {
          // Re-apply NOW so the stale composite (built for the old source/range)
          // stops painting immediately — `apply`'s sig guard falls back to the
          // per-tile TileLayer — instead of lingering on screen for the ~100ms the
          // rebuild takes. The matching composite is painted by the apply below.
          applyRef.current()
          const seq = ++buildSeq.current
          const built = await buildComposite(range, sources, build, () => seq !== buildSeq.current)
          if (!mounted.current || seq !== buildSeq.current) return // unmounted or superseded
          composite.current = built ? { ...built, sig } : null
        }
      }
      applyRef.current()
    })
  }, [mapRef, sources, build])

  // Re-run on source/highlight/overlay change. `overlay` is load-bearing: it's null
  // on first render, so the mount-time update() no-ops (apply guards on overlay);
  // re-firing once it lands paints the heatmap on load without needing a pan/zoom.
  useEffect(() => {
    update()
  }, [update, highlightGeometry, overlay])

  // Re-apply + (≥z14) rebuild the composite after the viewport settles.
  useEffect(() => {
    if (!mapRef) return
    const map = mapRef.getMap()
    map.on('moveend', update)
    return () => {
      map.off('moveend', update)
    }
  }, [mapRef, update])

  // Idle ancestor prefetch (owner ask 2026-07-16): once the viewport settles,
  // quietly pull the 3×3 ring of ancestor tiles around the view plus the
  // centre block one zoom deeper — memoized, low-priority, ≤10 × |sources|
  // small requests fired 600 ms after the map stops (deliberately outside
  // deck's request budget: they must never queue ahead of sharp tiles) — so
  // ANY pan or zoom direction already has its blurry preview in memory.
  const prefetchTimer = useRef(0)
  useEffect(() => {
    if (!mapRef || build === null || sources.length === 0) return
    const map = mapRef.getMap()
    const prefetchRing = () => {
      window.clearTimeout(prefetchTimer.current)
      prefetchTimer.current = window.setTimeout(() => {
        const z = Math.round(Math.min(Math.max(map.getZoom(), MIN_ZOOM), build.zoom))
        const pz = z - PREVIEW_DELTA
        if (pz < MIN_ZOOM) return
        const center = map.getCenter()
        const [fx, fy] = lngLatToTileFloat(center.lng, center.lat, pz)
        const span = 2 ** pz
        for (let dy = -1; dy <= 1; dy++) {
          const ay = Math.floor(fy) + dy
          if (ay < 0 || ay >= span) continue
          for (let dx = -1; dx <= 1; dx++) {
            const ax = (((Math.floor(fx) + dx) % span) + span) % span
            for (const s of sources) void fetchAncestor(tileUrl(build, s, pz, ax, ay))
          }
        }
        // Zoom-in ahead: the centre ancestor block ONE level deeper, so the
        // first wheel step also finds its preview waiting (zoom-out ancestors
        // are already cached from the way down). moveend fires after zooms
        // too, so the ring itself re-targets on every zoom change.
        if (pz + 1 <= build.zoom - PREVIEW_DELTA) {
          const deepSpan = 2 ** (pz + 1)
          const [zx, zy] = lngLatToTileFloat(center.lng, center.lat, pz + 1)
          const ax = ((Math.floor(zx) % deepSpan) + deepSpan) % deepSpan
          const ay = Math.floor(zy)
          if (ay >= 0 && ay < deepSpan) {
            for (const s of sources) void fetchAncestor(tileUrl(build, s, pz + 1, ax, ay))
          }
        }
      }, 600)
    }
    map.on('moveend', prefetchRing)
    prefetchRing()
    return () => {
      window.clearTimeout(prefetchTimer.current)
      map.off('moveend', prefetchRing)
    }
  }, [mapRef, build, sources])

  // Track the basemap's first label layer as the heatmap's z-anchor (beforeId).
  useEffect(() => {
    if (!mapRef) return
    const map = mapRef.getMap()
    const sync = () => {
      const layers = map.getStyle()?.layers
      // Anchor below the first label layer so labels draw on top. Standard +
      // satellite rename labels `_label-*`; the Positron fallback doesn't, so
      // fall back to the first symbol layer.
      const id = layers?.find((l) => l.id.startsWith('_label'))?.id
        ?? layers?.find((l) => l.type === 'symbol')?.id
      if (id !== labelAnchor.current) {
        labelAnchor.current = id
        applyRef.current()
      }
    }
    sync()
    map.on('styledata', sync)
    return () => {
      map.off('styledata', sync)
    }
  }, [mapRef])

  return null
}

/** Per-tile heatmap as a deck `TileLayer` (used below the over-zoom threshold).
 *  `build` is the generation SNAPSHOT this layer is constructed for — both the
 *  id (deck's cache key) and every fetch URL use it, so the layer can never mix
 *  generations even if the module-level build advances mid-flight. */
function makeHeatmapTileLayer(
  build: TileBuilds,
  sources: readonly HeatmapSource[],
  beforeId: string | undefined,
  refineSeq: number,
  onRefined: () => void,
  registry: { aborts: Set<() => void>; loaded: Set<string>; inFlight: Map<string, symbol> },
  zoomOffset: number,
) {
  const { aborts: tails, loaded, inFlight } = registry
  return new TileLayer<HeatTile | null>({
    id: `hm3-tiles-${buildKey(build, sources)}-o${zoomOffset}`,
    // beforeId on the TileLayer (NOT its sublayers): MapboxOverlay slots only the
    // top-level deck layer; the tile BitmapLayers draw inside it. Spread because
    // _TileLayerProps doesn't type beforeId though MapboxOverlay reads it at runtime.
    ...(beforeId ? { beforeId } : {}),
    minZoom: MIN_ZOOM,
    // The published base zoom is the native tile ceiling: deck requests no
    // index below it and over-zooms the deepest level itself past it.
    maxZoom: build.zoom,
    // +1 on HiDPI screens: one data cell ≈ one device pixel wherever a finer
    // pyramid level exists (maxZoom still clamps at the published data floor).
    zoomOffset,
    // Without an extent deck renders NOTHING once the computed tile zoom drops
    // below minZoom (world views under z≈1.5 were blank — owner report
    // 2026-07-16); with one it clamps to minZoom and scales the z2 world
    // tiles instead (deck getTileIndices contract).
    extent: WORLD_EXTENT,
    tileSize: TILE_PX,
    maxCacheSize: MAX_CACHE_TILES,
    // One fetch+decode per tile, so allow more in flight → faster fill.
    maxRequests: 12,
    // 'no-overlap', NOT 'best-available': the Lden palette is highly opaque, so
    // best-available's brief parent+child overlap during a zoom double-draws the
    // colour → a dark flash. no-overlap never overlaps them.
    refinementStrategy: 'no-overlap',
    getTileData: ({ index, signal }) => {
      const { x, y, z } = index
      const span = 2 ** z
      const wx = ((x % span) + span) % span // wrap x across the antimeridian
      const urls = sources.map((s) => tileUrl(build, s, z, wx, y))
      // Owner fallback policy 2026-07-16: exact zoom > deck's cached z−1 >
      // z−2 > … > coarse preview > blank. If ANY nearby ancestor is already
      // session-loaded, skip the preview — deck's no-overlap refinement keeps
      // painting that sharper ancestor until this tile's data lands (an empty
      // resolved ancestor counts too: blank IS its truth).
      const deckFallback = () => {
        for (let d = 1; d <= 6 && z - d >= MIN_ZOOM; d++) {
          if (loaded.has(`${z - d}/${wx >> d}/${y >> d}`)) return true
        }
        return false
      }
      const deckHasFallback = deckFallback()
      // z−Δ ancestor for the instant preview (shallow zooms are the warm band
      // itself — no preview needed or possible below the z2 floor).
      const pz = z - PREVIEW_DELTA
      const mask = (1 << PREVIEW_DELTA) - 1
      const preview = !deckHasFallback && pz >= MIN_ZOOM
        ? {
            urls: sources.map((s) => tileUrl(build, s, pz, wx >> PREVIEW_DELTA, y >> PREVIEW_DELTA)),
            blockX: wx & mask,
            blockY: y & mask,
          }
        : null
      const key = `${z}/${wx}/${y}`
      const token = Symbol(key)
      inFlight.set(key, token)
      return loadTileProgressively(urls, signal, onRefined, tails, preview, deckFallback).then((data) => {
        // Register only while THIS request is still the current one for the
        // key — a tile evicted while pending (or superseded by a newer
        // request) must not become a ghost ancestor.
        if (inFlight.get(key) === token) {
          inFlight.delete(key)
          loaded.add(key)
        }
        return data
      })
    },
    // Bumped by onRefined when a tile's late layers land — deck re-runs
    // renderSubLayers, and only mutated tiles carry a NEW ImageData reference
    // (unchanged tiles keep their texture; the sublayer descriptors of the
    // visible set are recreated, a few ms coalesced to one frame per wave).
    updateTriggers: { renderSubLayers: refineSeq },
    // deck can no longer abort a tile once it resolved (see
    // loadTileProgressively) — cancel the progressive tail ourselves when the
    // tile leaves the cache, so evicted tiles stop fetching and repainting.
    onTileUnload: (tile) => {
      const { x, y, z } = tile.index
      const span = 2 ** z
      const key = `${z}/${((x % span) + span) % span}/${y}`
      loaded.delete(key)
      inFlight.delete(key) // a pending resolve loses its token → never registers
      const d = tile.data
      if (!d) return
      // A still-pending tile can be evicted without deck aborting it — cancel
      // its tail the moment it resolves instead of letting it refine a ghost.
      if (d instanceof Promise) void d.then((t) => t?.abortRefine?.()).catch(() => {})
      else d.abortRefine?.()
    },
    renderSubLayers: (props) => {
      const data = props.data as HeatTile | null
      if (!data) return null
      const { west, south, east, north } = props.tile.bbox as {
        west: number; south: number; east: number; north: number
      }
      return new BitmapLayer({
        id: `${props.id}-bitmap`,
        image: data.image,
        bounds: [west, south, east, north],
        textureParameters: { minFilter: 'linear', magFilter: 'linear' },
      })
    },
  })
}

/**
 * Contributor-highlight casing + core pair: a wider black stroke under a thinner
 * white one (readable on any basemap). No `beforeId` → drawn last, above labels.
 */
function makeHighlightLayers(geometry: GeoJSON.Geometry | null | undefined) {
  if (!geometry) return []
  const data: GeoJSON.Feature = { type: 'Feature', geometry, properties: {} }
  return [
    new GeoJsonLayer({
      id: 'contributor-highlight-casing',
      data,
      stroked: true, filled: true,
      getLineColor: [0, 0, 0, 255],
      getLineWidth: 5, lineWidthMinPixels: 5, lineWidthUnits: 'pixels',
      getFillColor: [255, 255, 255, 76],
      getPointRadius: 5, pointRadiusUnits: 'pixels',
      pickable: false,
    }),
    new GeoJsonLayer({
      id: 'contributor-highlight-core',
      data,
      stroked: true, filled: false,
      getLineColor: [255, 255, 255, 255],
      getLineWidth: 2, lineWidthMinPixels: 2, lineWidthUnits: 'pixels',
      getPointRadius: 3, pointRadiusUnits: 'pixels',
      pickable: false,
    }),
  ]
}
