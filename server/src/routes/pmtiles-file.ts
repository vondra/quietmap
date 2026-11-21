// The one reader of PMTiles archives on disk: directories parsed, tile bytes handed out as stored.

import { constants } from 'node:fs'
import { open } from 'node:fs/promises'
import { promisify } from 'node:util'
import { gunzip } from 'node:zlib'
import {
  Compression,
  PMTiles,
  SharedPromiseCache,
  type RangeResponse,
  type Source,
} from 'pmtiles'

const gunzipAsync = promisify(gunzip)

/** Cache parsed directories, but retain an archive descriptor only during a range read:
 * an idle cached descriptor would pin hundreds of GB after publication GC unlinks it. */
class FileRangeSource implements Source {
  private readonly path: string

  constructor(path: string) {
    this.path = path
  }

  getKey(): string {
    return this.path
  }

  async getBytes(offset: number, length: number): Promise<RangeResponse> {
    const handle = await open(
      this.path,
      constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK,
    )
    try {
      if (!(await handle.stat()).isFile()) throw new Error(`${this.path} is not a regular file`)
      // Positional reads may be short; only EOF ends the range early. The header probe at the
      // file's start asks for 16 KiB even of a smaller archive; anywhere else a range that ends
      // early is a truncated archive, and part of a tile must never be served as the tile.
      const data = new ArrayBuffer(length)
      const view = new Uint8Array(data)
      let filled = 0
      while (filled < length) {
        const { bytesRead } = await handle.read(view, filled, length - filled, offset + filled)
        if (bytesRead === 0) break
        filled += bytesRead
      }
      if (filled === length) return { data }
      if (offset !== 0) throw new Error(`${this.path} ends inside the range ${offset}+${length}`)
      return { data: data.slice(0, filled) }
    } finally {
      await handle.close()
    }
  }
}

async function inflateDirectory(buf: ArrayBuffer, compression: Compression): Promise<ArrayBuffer> {
  if (compression === Compression.None) return buf
  if (compression !== Compression.Gzip) throw new Error(`unsupported internal compression ${compression}`)
  const out = await gunzipAsync(buf)
  const copy = new ArrayBuffer(out.byteLength)
  new Uint8Array(copy).set(out)
  return copy
}

/**
 * Opens an archive whose tiles a route ships verbatim under the `Content-Encoding` of
 * `tileCompression`: the browser inflates them in its network stack and the server stays a byte
 * reader. The pmtiles lib inflates directories with its cache's function and tiles with its own,
 * so the tiles' is the identity; an archive whose header names another tile compression is
 * rejected instead of ever being served under the wrong encoding. `cachedDirectories` bounds the
 * parsed directories kept in memory.
 */
export async function openArchiveServedAsStored(
  path: string,
  tileCompression: Compression,
  cachedDirectories: number,
): Promise<PMTiles> {
  const pmtiles = new PMTiles(
    new FileRangeSource(path),
    new SharedPromiseCache(cachedDirectories, true, inflateDirectory),
    async tile => tile,
  )
  const header = await pmtiles.getHeader()
  if (header.tileCompression !== tileCompression) {
    throw new Error(`tile compression ${header.tileCompression} contradicts serving as ${tileCompression}`)
  }
  return pmtiles
}
