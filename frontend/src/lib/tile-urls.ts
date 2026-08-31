// Versioned heatmap-tile URLs — tracks the published pmtiles generation
// ("build") from /api/tiles-manifest.
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
import { latLngToCell } from 'h3-js'

// The published tile world's zoom band — ONE source for every component
// (mirrors server heatmap-shared.ts and what the packer writes). Base level
// z12: 512-px tiles carrying the old z13-pixel lattice; EVERY layer pyramids
// down to z2 — a single-layer view at world zoom is a first-class use case
// (deck's TileLayer renders NOTHING below its minZoom, so a deeper floor
// blanked e.g. roads-only at z4 — owner report 2026-07-09).
export const BASE_ZOOM = 12
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
/** One published zoom-tier pack: its R4 coverage plus the
 *  layer tokens its archives serve. Packs are immutable; LATER packs own an
 *  R4 they share with an earlier one. */
export interface TierPack {
  pack: string
  coverage: ReadonlySet<string>
  layers: ReadonlySet<string>
}

export interface TileBuilds {
  latest: string
  byLayer: Record<string, string>
  /** Tile hostname prefix (manifest `tile_base`) —
   *  '' = same-origin. Deployment topology, delivered with the manifest so a
   *  serving move never needs a frontend rebuild. */
  base: string
  /** Published zoom tiers ("z13" → packs), empty when none. Rides the SAME
   *  snapshot as builds so deck layer ids / composite signatures re-key when
   *  tier coverage changes (the generation-snapshot contract above). */
  tiers: Record<string, TierPack[]>
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
 * Cache-identity string for a SET of sources: each source's own build. Deck
 * layer ids and composite signatures key on THIS, not on `latest` — so a
 * partial flip re-keys exactly the layers whose archives changed (unchanged
 * layers keep their deck tile cache), and a same-`latest` sequential partial
 * still re-keys the layer it republished.
 * Tier packs fold in as a suffix so coverage growth re-keys tile caches too.
 */
export function buildKey(builds: TileBuilds, sources: readonly string[]): string {
  const base = sources.map((s) => `${s}:${builds.byLayer[s] ?? builds.latest}`).join('|')
  const tiers = Object.entries(builds.tiers)
    .map(([zoom, packs]) => `${zoom}[${packs.map((p) => p.pack).join(',')}]`)
    .join('|')
  return tiers ? `${base}|tiers:${tiers}` : base
}

/** True when any tier pack is published — the overlay raises its native tile
 *  zoom only then, so the no-tier world keeps today's exact behavior. */
export function hasTierCoverage(builds: TileBuilds, zoom: string): boolean {
  return (builds.tiers[zoom]?.length ?? 0) > 0
}

/**
 * R4 hex of a display tile's centre — the EXACT engine coverage rule
 * (tile-painter region_runner::tile_centre_r4: arithmetic mean of the tile
 * bbox's lat/lon, then H3 res 4). A different centre definition would create
 * false-positive coverage and render authoritative silence over real z12
 * data, so keep this in lockstep with the Rust.
 */
export function tileCentreR4(z: number, x: number, y: number): string {
  const n = 2 ** z
  const lonW = (x / n) * 360 - 180
  const lonE = ((x + 1) / n) * 360 - 180
  const latN = (Math.atan(Math.sinh(Math.PI * (1 - (2 * y) / n))) * 180) / Math.PI
  const latS = (Math.atan(Math.sinh(Math.PI * (1 - (2 * (y + 1)) / n))) * 180) / Math.PI
  return latLngToCell((latN + latS) * 0.5, (lonW + lonE) * 0.5, 4)
}

/**
 * Resolve one display tile of `source` against the published tier packs:
 * the tier layer token to fetch natively, or `null` when the tile is outside
 * tier coverage (caller falls back to the z12 parent). The LAST pack
 * containing the tile's R4 owns the WHOLE R4 (append-order supersession) —
 * no fallback to an older pack for a missing layer: the packer always ships
 * the complete published layer set per pack, so a missing token in the
 * owner means a broken publish, and serving an older pack's tile there
 * would silently mix generations. A MISS on a returned token is
 * authoritative silence, never a fallback.
 */
export function tierTokenFor(
  builds: TileBuilds,
  source: string,
  z: number,
  x: number,
  y: number,
): string | null {
  const packs = builds.tiers[`z${z}`]
  if (!packs?.length) return null
  const r4 = tileCentreR4(z, x, y)
  for (let i = packs.length - 1; i >= 0; i--) {
    if (!packs[i].coverage.has(r4)) continue
    const token = `${source}-z${z}-${packs[i].pack}`
    return packs[i].layers.has(token) ? token : null
  }
  return null
}

/** The ONE fetch-plan contract shared by the tile layer,
 *  the over-zoom composite and the hover readout: either a native archive
 *  URL, or the z12 parent URL plus which quadrant of it this tile magnifies. */
export type TileFetchSpec =
  | { url: string }
  | { parentUrl: string; quadrant: { dx: 0 | 1; dy: 0 | 1 } }

/**
 * Fetch plan for one display tile of `source`:
 *  - base band (z ≤ 12): the ordinary archive URL;
 *  - tier zoom, covered: the pack's native tile (miss = authoritative silence);
 *  - tier zoom, uncovered: the z12 parent + quadrant to crop-upscale client-side.
 */
export function resolveTileFetch(
  builds: TileBuilds,
  source: string,
  z: number,
  x: number,
  y: number,
): TileFetchSpec {
  if (z <= BASE_ZOOM) return { url: tileUrl(builds, source, z, x, y) }
  // Single-shift parent below assumes z == BASE_ZOOM + 1 — the only tier the
  // UI activates (maxZoom clamps at 13; a z14 pack stays inert). Revisit the
  // quadrant math before ever raising that ceiling.
  const token = tierTokenFor(builds, source, z, x, y)
  if (token !== null) return { url: tileUrl(builds, token, z, x, y) }
  return {
    parentUrl: tileUrl(builds, source, BASE_ZOOM, x >> 1, y >> 1),
    quadrant: { dx: (x & 1) as 0 | 1, dy: (y & 1) as 0 | 1 },
  }
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
      tile_base?: unknown
      layers?: Record<string, { build?: unknown; file?: unknown }>
      tiers?: Record<string, { packs?: unknown }>
    }
    if (typeof manifest.build !== 'string' || !BUILD_ID.test(manifest.build)) return
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
    // Tier index (packs are metadata; their archives are ordinary layer
    // entries above). A malformed pack entry is SKIPPED, never trusted: a
    // wrong coverage set would render authoritative silence over real data —
    // so ids, coverage cells and layer tokens are validated by SHAPE, not
    // just by type (canonical p<N>; canonical lowercase res-4 H3 ids, which
    // always spell `84…ffffffff`; tokens that parse for this zoom + pack and
    // whose archives exist in `layers`).
    const tiers: Record<string, TierPack[]> = {}
    for (const [zoom, entry] of Object.entries(manifest.tiers ?? {})) {
      const zoomNum = /^z(1[3-8])$/.exec(zoom)?.[1]
      if (!zoomNum || !Array.isArray(entry?.packs)) continue
      const packs: TierPack[] = []
      for (const p of entry.packs as Array<Record<string, unknown>>) {
        if (
          typeof p?.pack !== 'string' || !/^p[0-9]+$/.test(p.pack)
          || !Array.isArray(p.coverage_r4)
          || !Array.isArray(p.layers)
          || !p.coverage_r4.every((c) => typeof c === 'string' && /^84[0-9a-f]{5}ffffffff$/.test(c))
          || !p.layers.every((l) => typeof l === 'string'
            && l.endsWith(`-z${zoomNum}-${p.pack}`)
            && typeof (manifest.layers ?? {})[l] === 'object')
        ) continue
        packs.push({
          pack: p.pack,
          coverage: new Set(p.coverage_r4 as string[]),
          layers: new Set(p.layers as string[]),
        })
      }
      if (packs.length) tiers[zoom] = packs
    }
    const next: TileBuilds = { latest: manifest.build, byLayer, base, tiers }
    // Sets are invisible to plain JSON.stringify — canonicalise them so a
    // tier coverage change reliably notifies subscribers.
    const canonical = (b: TileBuilds | null): string =>
      JSON.stringify(b, (_key, value) => (value instanceof Set ? [...value].sort() : value))
    if (canonical(next) !== canonical(currentBuilds)) {
      currentBuilds = next
      for (const cb of listeners) cb()
    }
  } catch {
    // Network hiccup — keep the current builds; the next poll retries.
  }
}
