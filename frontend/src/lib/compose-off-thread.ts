// Off-main-thread palette compose for the heatmap tile pipeline.
//
// The energy sum + palette map of a 512² tile costs ~2–8 ms of main-thread
// time per compose (40–90 ms per pan wave, visible as jank). This wrapper
// ships the grids to a dedicated worker via structured clone (~0.3 ms — grids
// stay usable on the main thread for progressive recomposes) and gets the
// pixels back as a zero-copy transfer.
//
// Falls back to a synchronous main-thread compose when Workers are
// unavailable, and drains in-flight requests synchronously if the worker
// dies mid-load — a dead worker must never strand a tile unpainted.

import { composeToImageData, upsampleAncestorBlock } from './hm3-compose'

type ComposeReply = { id: number; pixels: ArrayBuffer; width: number; height: number }
type Block = { x: number; y: number }
type Pending = {
  resolve: (img: ImageData) => void
  reject: (err: unknown) => void
  grids: Uint8Array[]
  width: number
  height: number
  block?: Block
}

function composeSync(grids: Uint8Array[], width: number, height: number, block?: Block): ImageData {
  const cells = block ? grids.map((g) => upsampleAncestorBlock(g, block.x, block.y)) : grids
  return composeToImageData(cells, width, height)
}

// undefined = not constructed yet · null = unavailable, use the sync fallback
let worker: Worker | null | undefined
const pending = new Map<number, Pending>()
let nextId = 0

function getWorker(): Worker | null {
  if (worker !== undefined) return worker
  try {
    worker = new Worker(new URL('./compose-off-thread.worker.ts', import.meta.url), {
      type: 'module',
    })
    worker.onmessage = (e: MessageEvent<ComposeReply>) => {
      const { id, pixels, width, height } = e.data
      const p = pending.get(id)
      if (!p) return
      pending.delete(id)
      p.resolve(new ImageData(new Uint8ClampedArray(pixels), width, height))
    }
    worker.onerror = () => {
      worker?.terminate()
      worker = null
      // Per-entry try/catch: if the compose itself is what killed the worker
      // (e.g. an allocation failure), the sync retry throws too — that entry
      // must reject rather than strand every later request behind it.
      for (const p of pending.values()) {
        try {
          p.resolve(composeSync(p.grids, p.width, p.height, p.block))
        } catch (err) {
          p.reject(err)
        }
      }
      pending.clear()
    }
  } catch {
    worker = null
  }
  return worker
}

/** Energy-sum + palette-map `grids` into a `width × height` ImageData off the
 *  main thread; synchronous on-thread fallback when no worker is available.
 *  Callers pass a SNAPSHOT (`grids.slice()`) when the array can still grow.
 *  With `block`, `grids` are z−PREVIEW_DELTA ancestor grids and the worker
 *  upsamples this child's sub-block of each before composing (preview). */
export function composeOffThread(
  grids: Uint8Array[],
  width: number,
  height: number,
  block?: Block,
): Promise<ImageData> {
  const w = getWorker()
  if (!w) return Promise.resolve(composeSync(grids, width, height, block))
  return new Promise((resolve, reject) => {
    const id = nextId++
    pending.set(id, { resolve, reject, grids, width, height, block })
    w.postMessage({ id, grids, width, height, block })
  })
}
