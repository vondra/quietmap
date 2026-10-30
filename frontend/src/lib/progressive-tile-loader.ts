// Tile loading with an ancestor preview — the React/deck-independent state
// machine behind HeatmapOverlay's getTileData (head-start race, memoized
// ancestor preview with grace, complete-for-complete swap, the sharp tile's
// late swap). Timing constants live here, next to the races they govern.

import { TILE_PX } from './hm3-decoder'
import { fetchSourceTile, type SourceTile } from './hm3-combine'
import { composeOffThread } from './compose-off-thread'

export type HeatTile = {
  image: ImageData
  /** Cancels the progressive tail (fetches deck can no longer abort — it
   *  disarms its own abort at our early resolve). Wired to onTileUnload. */
  abortRefine?: () => void
}

// Head start before a slow tile resolves early with its preview: on a warm CDN
// the tile lands inside this window, deck keeps its whole normal lifecycle
// (request throttling + aborts, zero progressive overhead) — only genuinely
// slow loads resolve early and swap in the sharp tile when it lands.
const EARLY_RESOLVE_AFTER_MS = 250

// Grace period before the ancestor preview may win a paint race: sharp tiles
// arriving under this paint directly — the blurred preview appears only where
// loads are GENUINELY slow. Applied in every race, measured from load start.
const PREVIEW_GRACE_MS = 400

// Upper bound on the complete-for-complete wait when deck is painting a
// cached sharper ancestor: past it the loader degrades to progressive rather
// than letting one hanging fetch pin the ancestor (and a deck request slot)
// forever.
const COMPLETE_SWAP_MAX_WAIT_MS = 4000

// Ancestor fetches are memoized: a whole block of children (4^Δ tiles) shares
// one ancestor and the browser does NOT coalesce concurrent fetches of the
// same URL — without this a cold wave fired a duplicate ancestor request per
// tile. Shared resource, so deliberately NOT tied to any one tile's
// abort; 'low' priority keeps sharp tiles ahead in the network queue.
type AncestorEntry = { promise: Promise<SourceTile | null>; done: boolean }
const ancestorMemo = new Map<string, AncestorEntry>()
const ANCESTOR_MEMO_MAX = 128
export function fetchAncestor(url: string): Promise<SourceTile | null> {
  const hit = ancestorMemo.get(url)
  if (hit) {
    // True LRU: refresh recency on every hit.
    ancestorMemo.delete(url)
    ancestorMemo.set(url, hit)
    return hit.promise
  }
  const entry: AncestorEntry = {
    promise: fetchSourceTile(url, undefined, 'low').catch(() => {
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

/**
 * A tile's load: the sharp tile gets a short head start; past it, while it is
 * still loading, the z−Δ ANCESTOR (Δ = PREVIEW_DELTA) paints as a preview
 * (one ancestor serves 4^Δ children, browser-cached, so a cold-area tile paints
 * a smooth coarse field ~instantly instead of nothing) and the sharp tile
 * replaces it when it lands, swapped on the same tile object (the caller's
 * `onRefined` bumps deck's repaint trigger). A preview with nothing real behind
 * it clears to transparent once the tile settles without levels (a preview
 * must never lie permanently). A failed tile with nothing shown fails, so deck
 * can refetch it.
 *
 * Lifecycle: deck releases its request slot and disarms its abort the moment
 * we resolve, and it never calls onTileUnload when finalizing a whole
 * layer — so the late fetch runs on our OWN controller, registered in
 * `tails`: cancelled by onTileUnload (eviction) or by the component when the
 * tile layer itself is swapped out.
 */
export async function loadTileProgressively(
  url: string,
  deckSignal: AbortSignal | undefined,
  onRefined: () => void,
  tails: Set<() => void>,
  previewSpec: { url: string; blockX: number; blockY: number } | null,
  hasDeckFallback?: () => boolean,
): Promise<HeatTile | null> {
  const ctl = new AbortController()
  if (deckSignal?.aborted) ctl.abort()
  deckSignal?.addEventListener('abort', () => ctl.abort())
  // The preview has its OWN controller: it dies with the tile (chained), and
  // is also cancelled the moment it becomes useless — the sharp tile landed —
  // without touching the sharp fetch.
  const previewCtl = new AbortController()
  if (ctl.signal.aborted) previewCtl.abort() // the chained listener below can't fire retroactively
  ctl.signal.addEventListener('abort', () => previewCtl.abort())
  // The decoder types the outcome: empty 200 body = tile without levels →
  // null; HTTP/network error = throw.
  let tile = null as SourceTile | null
  let failure: unknown = null
  let settled = false
  // 'high': sharp tiles outrank basemap assets and the 'low' ancestor
  // previews in Chromium's network queue.
  const sharp = fetchSourceTile(url, ctl.signal, 'high')
    .then((d) => {
      tile = d
      if (tile) previewCtl.abort() // real data beats any preview from here on
    })
    .catch((e) => {
      if (!ctl.signal.aborted) failure = e
    })
    .finally(() => { settled = true })
  let previewImage: ImageData | null = null
  const previewReady = new Promise<void>((resolve) => {
    const spec = previewSpec
    if (!spec) return
    void (async () => {
      const ancestor = await fetchAncestor(spec.url)
      if (previewCtl.signal.aborted || !ancestor?.cells) return
      // The worker upsamples the ancestor's sub-block AND composes — the main
      // thread only ships the raw ancestor grid.
      previewImage = await composeOffThread(ancestor.cells, ancestor.metric, TILE_PX, TILE_PX, {
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
  await Promise.race([sharp, delay(EARLY_RESOLVE_AFTER_MS), previewEligible])
  if (!settled && hasDeckFallback?.()) {
    // deck is already painting a cached sharper ancestor for this tile — an
    // early resolve with the coarse preview would visibly DOWNGRADE it
    // mid-zoom. Hold out (bounded) for the sharp tile; the ancestor stays up
    // meanwhile and the swap is complete-for-complete.
    await Promise.race([sharp, delay(COMPLETE_SWAP_MAX_WAIT_MS)])
  }
  if (!settled) {
    // Slow load: proceed once the sharp tile or the preview is ready.
    await Promise.race([sharp, previewEligible])
    if (!settled && hasDeckFallback?.()) {
      // A sharper cached ancestor landed while the preview was in flight —
      // resolving coarse now would HIDE it. Drop the preview and wait.
      previewImage = null
      await sharp
    }
  }
  if (ctl.signal.aborted) throw new DOMException('tile aborted', 'AbortError')
  if (settled) {
    // Complete load (fast path): plain tile, no tail to manage.
    previewCtl.abort()
    if (!tile) {
      if (failure !== null) throw failure
      return null
    }
    const image = await composeOffThread(tile.cells, tile.metric, TILE_PX, TILE_PX)
    if (ctl.signal.aborted) throw new DOMException('tile aborted', 'AbortError')
    return { image }
  }
  const abortTail = () => ctl.abort()
  tails.add(abortTail)
  // The preview fetch is a SHARED memoized resource that must not pin
  // per-tile closures in the registry: deregister when the sharp tile settles.
  void sharp.then(() => tails.delete(abortTail))
  // The race guarantees the preview here (transparent fallback is defensive).
  const data: HeatTile = { image: previewImage ?? new ImageData(TILE_PX, TILE_PX), abortRefine: abortTail }
  void sharp.then(async () => {
    if (ctl.signal.aborted) return
    previewCtl.abort()
    data.image = tile ? await composeOffThread(tile.cells, tile.metric, TILE_PX, TILE_PX) : new ImageData(TILE_PX, TILE_PX)
    if (!ctl.signal.aborted) onRefined()
  }).catch(() => { /* the preview stays; a later load refetches */ })
  return data
}
