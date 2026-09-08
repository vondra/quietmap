// Web Worker entry: stateless palette compose — energy-sum + palette-map runs
// off the UI thread. Input arrives structured-cloned ({id, grids, width,
// height}); the composed pixels transfer back zero-copy. All session state
// and fallback logic live in the client (compose-off-thread.ts).

import { composeToImageData, upsampleAncestorBlock } from './hm3-compose'

type ComposeRequest = {
  id: number
  grids: Uint8Array[]
  width: number
  height: number
  /** When set, `grids` are z−PREVIEW_DELTA ANCESTOR grids: upsample this
   *  child's sub-block of each before composing (off the main thread too). */
  block?: { x: number; y: number }
}

const ctx = self as unknown as {
  onmessage: ((e: MessageEvent<ComposeRequest>) => void) | null
  postMessage: (msg: unknown, transfer: Transferable[]) => void
}

ctx.onmessage = (e) => {
  const { id, grids, width, height, block } = e.data
  const cells = block ? grids.map((g) => upsampleAncestorBlock(g, block.x, block.y)) : grids
  const image = composeToImageData(cells, width, height)
  ctx.postMessage({ id, pixels: image.data.buffer, width, height }, [image.data.buffer])
}
