// Progressive multi-layer tile loading — the React/deck-independent state
// machine behind HeatmapOverlay's getTileData (head-start race, memoized
// ancestor preview with grace, complete-for-complete swap, per-frame
// recompose tail). Timing constants live here, next to the races they govern.

import { fetchAndDecodeHM3, TILE_PX } from './hm3-decoder'
import { composeOffThread } from './compose-off-thread'
import type { TileFetchSpec } from './tile-urls'

export type HeatTile = {
  image: ImageData
  /** Cancels the progressive tail (fetches deck can no longer abort — it
   *  disarms its own abort at our early resolve). Wired to onTileUnload. */
  abortRefine?: () => void
}

// Head start before a partially-loaded tile resolves early: on a warm CDN the
// full layer set lands inside this window, deck keeps its whole normal
// lifecycle (request throttling + aborts, zero progressive overhead) — only
// genuinely slow loads resolve early and refine as layers land. Their tails
// escape deck's maxRequests budget by design: they are same-viewport work
// that still fills the tile cache, they are bounded by the wave size, and
// every cancellation path (eviction, re-key, unmount) can reach them —
// enforcing the budget exactly would mean reimplementing deck's scheduler.
const EARLY_RESOLVE_AFTER_MS = 250

// Grace period before the ancestor preview may win a paint race: sharp tiles
// arriving under this paint directly — the blurred preview appears only where
// loads are GENUINELY slow (owner report 2026-07-16: a memo-resolved preview
// was instantly beating ~300 ms tiles, flashing blur over areas that were
// about to be sharp). Applied in every race, measured from load start.
const PREVIEW_GRACE_MS = 400

// Upper bound on the complete-for-complete wait when deck is painting a
// cached sharper ancestor: past it the loader degrades to progressive rather
// than letting one hanging fetch pin the ancestor (and a deck request slot)
// forever.
const COMPLETE_SWAP_MAX_WAIT_MS = 4000

// Ancestor fetches are memoized: a whole block of children (4^Δ tiles) shares
// one ancestor and the browser does NOT coalesce concurrent fetches of the
// same URL — without this a cold wave fired a duplicate ancestor request per
// tile (owner report 2026-07-16). Shared resource, so deliberately NOT tied to any one tile's
// abort; 'low' priority keeps sharp tiles ahead in the network queue. (deck's
// own sharp-path fetch of the same URL after a deep zoom-out can still
// duplicate one request — rare, lands on the warmed CDN band, accepted.)
type AncestorEntry = { promise: Promise<{ cells: Uint8Array } | null>; done: boolean }
const ancestorMemo = new Map<string, AncestorEntry>()
const ANCESTOR_MEMO_MAX = 128
export function fetchAncestor(url: string): Promise<{ cells: Uint8Array } | null> {
  const hit = ancestorMemo.get(url)
  if (hit) {
    // True LRU: refresh recency on every hit.
    ancestorMemo.delete(url)
    ancestorMemo.set(url, hit)
    return hit.promise
  }
  const entry: AncestorEntry = {
    promise: fetchAndDecodeHM3(url, undefined, 'low').catch(() => {
      // A failed fetch must not negative-cache as "empty ancestor".
      ancestorMemo.delete(url)
      return null
    }),
    done: false,
  }
  void entry.promise.finally(() => { entry.done = true })
  ancestorMemo.set(url, entry)
  if (ancestorMemo.size > ANCESTOR_MEMO_MAX) {
    // Evict the oldest SETTLED entry — never an in-flight promise another
    // tile is about to share (in-flight count bounds any temporary overshoot).
    for (const [key, e] of ancestorMemo) {
      if (e.done) {
        ancestorMemo.delete(key)
        break
      }
    }
  }
  return entry.promise
}

/** Nearest-neighbour 2× upscale of one parent quadrant onto a full tile grid:
 *  display
 *  tiles OUTSIDE tier coverage magnify their z12 parent's quadrant, exactly
 *  like deck's own overzoom, but on the byte grid so the energy sum and
 *  palette stay identical to the base band. NO_DATA passes through. */
export function upscaleQuadrant(
  cells: Uint8Array,
  quadrant: { dx: 0 | 1; dy: 0 | 1 },
): Uint8Array {
  const half = TILE_PX / 2
  const out = new Uint8Array(TILE_PX * TILE_PX)
  const ox = quadrant.dx * half
  const oy = quadrant.dy * half
  for (let y = 0; y < TILE_PX; y++) {
    const srcRow = (oy + (y >> 1)) * TILE_PX
    const outRow = y * TILE_PX
    for (let x = 0; x < TILE_PX; x++) {
      out[outRow + x] = cells[srcRow + ox + (x >> 1)]
    }
  }
  return out
}

/** Execute one fetch-plan entry: a native archive tile, or the z12 parent
 *  crop-upscaled. Shared by the tile layer and the over-zoom composite.
 *  Parent fetches go through the ancestor memo: four uncovered z13 siblings
 *  share ONE z12 parent, and the browser does not coalesce concurrent
 *  same-URL fetches (the memo's own rationale) — without this the composite
 *  fires 4× duplicate parent fetch+decodes (gg z13 impl review). The memo's
 *  'low' priority is acceptable: the parent is almost always browser-cached
 *  from the z12 view the user just zoomed through. Known trade-off: the
 *  memo's failure shape is `null` (it never negative-caches — a failed
 *  fetch evicts itself), so a hard PARENT failure renders as empty until
 *  the next request rather than failing the tile; typed errors matter most
 *  for NATIVE tier tiles (inside coverage), which keep the throwing path. */
export async function fetchSpecGrid(
  spec: TileFetchSpec,
  signal: AbortSignal | undefined,
  priority: 'high' | 'low',
): Promise<{ cells: Uint8Array } | null> {
  if ('url' in spec) return fetchAndDecodeHM3(spec.url, signal, priority)
  const parent = await fetchAncestor(spec.parentUrl)
  if (!parent?.cells) return null
  return { cells: upscaleQuadrant(parent.cells, spec.quadrant) }
}

/**
 * Progressive multi-layer tile load: give the full set a short head start;
 * past it, resolve with whatever landed FIRST so the tile paints at the
 * fastest layer's latency, then recompose as the remaining layers land
 * (coalesced to one compose per frame per tile) by swapping the image on the
 * same tile object — the caller's `onRefined` bumps deck's repaint trigger.
 * The partial energy sum transiently underestimates — a lighter shade for a
 * moment beats a blank tile — and converges to the exact sum with the last
 * layer. A failed single layer renders as that layer being empty
 * (pre-existing behavior).
 *
 * While the real layers load, the z−Δ ANCESTOR (Δ = PREVIEW_DELTA) races
 * them as a preview: one ancestor serves 4^Δ children (browser-cached) and
 * the warm-crawled z≤9 band keeps it an edge HIT, so a cold-area tile paints
 * a smooth coarse energy field ~instantly instead of nothing. The first real
 * compose replaces it; a preview with nothing real behind it clears to
 * transparent once every layer settles empty (a preview must never lie
 * permanently).
 *
 * Lifecycle: deck releases its request slot and disarms its abort the moment
 * we resolve (tile-2d-header marks the tile loaded), and it never calls
 * onTileUnload when finalizing a whole layer — so the tail fetches run on our
 * OWN controller, registered in `tails`: cancelled by onTileUnload (eviction)
 * or by the component when the tile layer itself is swapped out.
 */
export async function loadTileProgressively(
  specs: readonly TileFetchSpec[],
  deckSignal: AbortSignal | undefined,
  onRefined: () => void,
  tails: Set<() => void>,
  previewSpec: { urls: string[]; blockX: number; blockY: number } | null,
  hasDeckFallback?: () => boolean,
): Promise<HeatTile | null> {
  const ctl = new AbortController()
  if (deckSignal?.aborted) ctl.abort()
  deckSignal?.addEventListener('abort', () => ctl.abort())
  // The preview has its OWN controller: it dies with the tile (chained), and
  // is also cancelled the moment it becomes useless — a real grid landed, or
  // the load completed — without touching the real fetches.
  const previewCtl = new AbortController()
  if (ctl.signal.aborted) previewCtl.abort() // the chained listener below can't fire retroactively
  ctl.signal.addEventListener('abort', () => previewCtl.abort())
  const grids: Uint8Array[] = []
  // Transient-vs-authoritative (gg z13 impl review, Codex #7): the decoder
  // types them (empty 200 body = authoritative silence → null; HTTP/network
  // error = throw). A single failed layer still renders as that layer being
  // empty (pre-existing best-effort), but a tile where EVERY layer hard-
  // failed must FAIL (deck can refetch) instead of caching silence.
  let hardErrors = 0
  let firstError: unknown = null
  // 'high': sharp tiles outrank basemap assets and the 'low' ancestor
  // previews in Chromium's network queue.
  const perFetch = specs.map((spec) =>
    fetchSpecGrid(spec, ctl.signal, 'high')
      .then((d) => {
        if (d?.cells) {
          grids.push(d.cells)
          previewCtl.abort() // real data beats any preview from here on
        }
      })
      .catch((e) => {
        if (!ctl.signal.aborted) {
          hardErrors += 1
          firstError ??= e
        }
      }),
  )
  const allDone = Promise.all(perFetch)
  let allSettled = false
  void allDone.then(() => { allSettled = true })
  let previewImage: ImageData | null = null
  const previewReady = new Promise<void>((resolve) => {
    const spec = previewSpec
    if (!spec) return
    void (async () => {
      const decoded = await Promise.all(spec.urls.map((u) => fetchAncestor(u)))
      if (previewCtl.signal.aborted) return
      const ancestors = decoded.flatMap((d) => (d?.cells ? [d.cells] : []))
      if (ancestors.length === 0 || previewCtl.signal.aborted) return
      // The worker upsamples each ancestor's sub-block AND composes — the
      // main thread only ships the raw ancestor grids.
      previewImage = await composeOffThread(ancestors, TILE_PX, TILE_PX, {
        x: spec.blockX,
        y: spec.blockY,
      })
      resolve()
    })().catch(() => { /* a failed preview just never shows */ })
  })
  const delay = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms))
  // The preview may only win a race after the grace period — sharp data
  // arriving under it paints directly, no blur flash.
  const previewEligible = delay(PREVIEW_GRACE_MS).then(() => previewReady)
  await Promise.race([allDone, delay(EARLY_RESOLVE_AFTER_MS), previewEligible])
  if (!allSettled && hasDeckFallback?.()) {
    // deck is already painting a cached sharper ancestor for this tile — an
    // early partial resolve would visibly DOWNGRADE it mid-zoom, each tile at
    // a different moment (patchwork jumping, owner report 2026-07-16). Hold
    // out (bounded) for the complete set; the ancestor stays up meanwhile and
    // the swap is complete-for-complete.
    await Promise.race([allDone, delay(COMPLETE_SWAP_MAX_WAIT_MS)])
  }
  if (!allSettled) {
    // Slow load → progressive: wake on every settle, proceed once ANY real
    // grid landed, the ancestor preview is ready, or everything settled.
    const firstGrid = new Promise<void>((resolve) => {
      for (const p of perFetch) void p.then(() => { if (grids.length > 0) resolve() })
    })
    await Promise.race([allDone, previewEligible, firstGrid])
    if (!allSettled && grids.length === 0 && previewImage && hasDeckFallback?.()) {
      // A sharper cached ancestor landed while the preview was in flight —
      // resolving coarse now would HIDE it (no-overlap shows the exact tile
      // once loaded). Drop the preview and wait for real data instead.
      previewImage = null
      await Promise.race([allDone, firstGrid])
    }
    if (!allSettled && grids.length > 0 && hasDeckFallback?.()) {
      // The ancestor can also land while waiting above — re-check before a
      // PARTIAL resolve would overpaint it (bounded like the branch above).
      await Promise.race([allDone, delay(COMPLETE_SWAP_MAX_WAIT_MS)])
    }
  }
  if (ctl.signal.aborted) throw new DOMException('tile aborted', 'AbortError')
  if (allSettled) {
    // Complete load (fast path): plain tile, no tail to manage.
    previewCtl.abort()
    if (grids.length === 0) {
      // Nothing landed AND every miss was a hard error → surface the error
      // so deck marks the tile failed and can refetch, instead of caching
      // transparent "silence" for the session. All-authoritative-empty
      // (hardErrors == 0) stays a legitimate empty tile.
      if (hardErrors === specs.length && firstError !== null) throw firstError
      return null
    }
    const image = await composeOffThread(grids, TILE_PX, TILE_PX)
    if (ctl.signal.aborted) throw new DOMException('tile aborted', 'AbortError')
    return { image }
  }
  const abortTail = () => ctl.abort()
  tails.add(abortTail)
  // Deregister on allDone only: the preview fetch is a SHARED memoized
  // resource that must not pin per-tile closures in the registry; a late
  // preview compose is already gated by previewCtl.signal.
  void allDone.then(() => tails.delete(abortTail))
  let composedCount = grids.length
  let scheduled = false
  // Real data when any landed; otherwise the race guarantees the preview
  // (transparent fallback is defensive only).
  const image = composedCount > 0
    ? await composeOffThread(grids.slice(0, composedCount), TILE_PX, TILE_PX)
    : previewImage ?? new ImageData(TILE_PX, TILE_PX)
  if (ctl.signal.aborted) throw new DOMException('tile aborted', 'AbortError')
  const data: HeatTile = { image, abortRefine: abortTail }
  const recomposeSoon = () => {
    if (scheduled) return
    scheduled = true
    requestAnimationFrame(() => {
      scheduled = false
      if (ctl.signal.aborted) return
      if (grids.length === composedCount) {
        // Preview with nothing real behind it: once every layer settled
        // empty, clear to a transparent tile instead of lying forever.
        if (allSettled && grids.length === 0) {
          previewCtl.abort()
          data.image = new ImageData(TILE_PX, TILE_PX)
          onRefined()
        }
        return
      }
      // composedCount only advances on SUCCESS — a rejected compose (worker
      // death mid-OOM) stays retryable by the next settle.
      const target = grids.length
      composeOffThread(grids.slice(0, target), TILE_PX, TILE_PX)
        .then((refined) => {
          if (ctl.signal.aborted) return
          composedCount = target
          data.image = refined
          onRefined()
        })
        .catch(() => { /* retried on the next settle */ })
    })
  }
  for (const p of perFetch) void p.then(recomposeSoon)
  return data
}
