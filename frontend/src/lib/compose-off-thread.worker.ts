// Web Worker entry: stateless palette compose — the palette map runs off the UI
// thread. Input arrives structured-cloned ({id, cells, width, height}); the
// composed pixels transfer back zero-copy. All session state and fallback logic
// live in the client (compose-off-thread.ts).

import { composeToImageData, upsampleAncestorBlock } from './hm3-compose'

type ComposeRequest = {
  id: number
  cells: Uint8Array
  width: number
  height: number
  /** When set, `cells` are a z−PREVIEW_DELTA ANCESTOR grid: upsample this
   *  child's sub-block of it before composing (off the main thread too). */
  block?: { x: number; y: number }
}

const ctx = self as unknown as {
  onmessage: ((e: MessageEvent<ComposeRequest>) => void) | null
  postMessage: (msg: unknown, transfer: Transferable[]) => void
}

ctx.onmessage = (e) => {
  const { id, cells, width, height, block } = e.data
  const image = composeToImageData(block ? upsampleAncestorBlock(cells, block.x, block.y) : cells, width, height)
  ctx.postMessage({ id, pixels: image.data.buffer, width, height }, [image.data.buffer])
}
