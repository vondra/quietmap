// Versioned heatmap-tile URLs — tracks the published pmtiles generation
// ("build") and the published base zoom from /api/tiles-manifest.
//
// GENERATION-SNAPSHOT CONTRACT: the
// build is delivered to components ONLY as a subscribed snapshot
// (`useTileBuild`) and passed EXPLICITLY into `tileUrl` — never read from
// module state inside a fetch closure. Every deck.gl layer id / tile cache is
// therefore keyed by exactly the generation it was constructed for: a
// mid-session flip re-renders (store notification) and re-keys the layers,
// instead of old layer instances silently fetching new-build tiles into their
// old caches (mixed generations on one screen).
//
// No published build (the ~50 ms before the manifest resolves, or a fresh
// checkout that never packed) = `null` = the tile layers simply don't mount.
// There is deliberately NO legacy-URL fallback — the loose-file route is gone.

import { useSyncExternalStore } from 'react'

// EVERY layer pyramids down to z2 — a single-layer view at world zoom is a
// first-class use case (deck's TileLayer renders NOTHING below its minZoom, so
// a deeper floor blanked e.g. roads-only at z4 — owner report 2026-07-09).
// The TOP of the band is not a constant: it is the zoom the published world was
// painted at, delivered per manifest as `TileBuilds.zoom`.
export const MIN_ZOOM = 2

// Web-Mercator world bounds for the tile layers' `extent` prop — deck only
// under-zooms (clamps a too-low tile zoom up to minZoom) when an extent is
// present; without one, world views below z≈1.5 render nothing at all.
export const WORLD_EXTENT: [number, number, number, number] = [-180, -85.051129, 180, 85.051129]

const BUILD_ID = /^b\d+$/
const FILE_BUILD = /\.(b\d+)\.pmtiles$/
const MANIFEST_POLL_MS = 10 * 60 * 1000

/** The published generation set: `latest` labels the manifest as a whole and
 *  keys deck layer ids / composite signatures; `byLayer` carries each layer's
 *  OWN build — a partial republish (e.g. road-only b3 over a b2 world) flips
 *  only that layer's URLs, and the untouched layers re-fetch straight from
 *  the browser's immutable cache. */
export interface TileBuilds {
  latest: string
  byLayer: Record<string, string>
  /** Tile hostname prefix (manifest `tile_base`) —
   *  '' = same-origin. Deployment topology, delivered with the manifest so a
   *  serving move never needs a frontend rebuild. */
  base: string
  /** The zoom the published world was painted at: the deepest level that
   *  exists natively, everything below it a pyramid level of the same paint.
   *  Rides the SAME snapshot as the builds, so a republish that changes it
   *  re-keys every deck layer id and composite signature. */
  zoom: number
}

let currentBuilds: TileBuilds | null = null
const listeners = new Set<() => void>()

/** The current tile generations, or `null` before any manifest resolves —
 *  subscribe, snapshot, pass down; render no tile layers while `null`. */
export function useTileBuild(): TileBuilds | null {
  return useSyncExternalStore(subscribe, snapshot)
}

function subscribe(cb: () => void): () => void {
  listeners.add(cb)
  return () => listeners.delete(cb)
}

function snapshot(): TileBuilds | null {
  return currentBuilds
}

/**
 * URL for one HM3 tile of `source` (a layer id or 'total') — the caller
 * passes the snapshot its layer was constructed with; the URL carries the
 * SOURCE's own build so partial republishes stay per-layer exact.
 */
export function tileUrl(builds: TileBuilds, source: string, z: number, x: number, y: number): string {
  const b = builds.byLayer[source] ?? builds.latest
  return `${builds.base}/api/tiles/${b}/${source}/${z}/${x}/${y}.bin`
}

/**
 * Cache-identity string for a SET of sources: each source's own build, plus the
 * published base zoom. Deck layer ids and composite signatures key on THIS, not
 * on `latest` — so a partial flip re-keys exactly the layers whose archives
 * changed (unchanged layers keep their deck tile cache), and a same-`latest`
 * sequential partial still re-keys the layer it republished.
 */
export function buildKey(builds: TileBuilds, sources: readonly string[]): string {
  const base = sources.map((s) => `${s}:${builds.byLayer[s] ?? builds.latest}`).join('|')
  return `${base}|z${builds.zoom}`
}

/**
 * Resolve and track the current build. Fire once (non-blocking) at app boot;
 * re-polls every 10 minutes and when the tab becomes visible again.
 *
 * A session never downgrades to `null` once a build is known: a published
 * generation is immutable and stays servable, so a later manifest 404/error
 * keeps the last known build.
 */
export async function initTileBuild(): Promise<void> {
  if (pollingStarted) return
  pollingStarted = true
  setInterval(() => void refreshTileBuild(), MANIFEST_POLL_MS)
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'visible') void refreshTileBuild()
  })
  await refreshTileBuild()
}

let pollingStarted = false

async function refreshTileBuild(): Promise<void> {
  try {
    const res = await fetch('/api/tiles-manifest', { cache: 'no-cache' })
    if (!res.ok) return // nothing published yet → stay as-is
    const manifest = (await res.json()) as {
      build?: unknown
      zoom?: unknown
      tile_base?: unknown
      layers?: Record<string, { build?: unknown; file?: unknown }>
    }
    if (typeof manifest.build !== 'string' || !BUILD_ID.test(manifest.build)) return
    // A manifest without a usable base zoom is not servable at all: every tile
    // request needs the top of the band. Keep the current snapshot instead.
    if (!Number.isInteger(manifest.zoom) || (manifest.zoom as number) < MIN_ZOOM) return
    const byLayer: Record<string, string> = {}
    for (const [layer, entry] of Object.entries(manifest.layers ?? {})) {
      // Per-layer build straight from the entry; pre-partial-pack manifests
      // lack it — recover it from the archive filename instead.
      const b =
        typeof entry.build === 'string' && BUILD_ID.test(entry.build)
          ? entry.build
          : typeof entry.file === 'string'
            ? FILE_BUILD.exec(entry.file)?.[1]
            : undefined
      byLayer[layer] = b ?? manifest.build
    }
    // Only an https/relative-safe absolute base is accepted — anything odd
    // degrades to same-origin rather than sending tile traffic somewhere weird.
    const base = typeof manifest.tile_base === 'string' && /^https:\/\/[a-z0-9.-]+$/i.test(manifest.tile_base)
      ? manifest.tile_base
      : ''
    const next: TileBuilds = { latest: manifest.build, byLayer, base, zoom: manifest.zoom as number }
    if (JSON.stringify(next) !== JSON.stringify(currentBuilds)) {
      currentBuilds = next
      for (const cb of listeners) cb()
    }
  } catch {
    // Network hiccup — keep the current builds; the next poll retries.
  }
}
