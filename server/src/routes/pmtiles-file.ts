// The one reader of PMTiles archives on disk: directories parsed, tile bytes handed out as stored.

import { constants } from 'node:fs'
import { lstat, open } from 'node:fs/promises'
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

export class ArchiveChangedError extends Error {
  constructor(message: string) {
    super(message)
    this.name = 'ArchiveChangedError'
  }
}

interface FileIdentity {
  dev: number
  ino: number
}

export interface VerifiedArchive {
  pmtiles: PMTiles
  /** Verify the pathname still resolves to the exact regular file validated at open time. */
  verifyIdentity: () => Promise<void>
}

/** Cache parsed directories, but retain an archive descriptor only during a range read:
 * an idle cached descriptor would pin hundreds of GB after publication GC unlinks it.
 * Every open verifies `(dev, ino)` so an in-place swap can never feed another file's bytes
 * to cached directory offsets. */
class FileRangeSource implements Source {
  private readonly path: string
  private readonly identity: FileIdentity

  constructor(path: string, identity: FileIdentity) {
    this.path = path
    this.identity = identity
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
      const stat = await handle.stat()
      if (!stat.isFile()) throw new Error(`${this.path} is not a regular file`)
      if (stat.dev !== this.identity.dev || stat.ino !== this.identity.ino) {
        throw new ArchiveChangedError(`${this.path} was replaced after validation`)
      }
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
 * `tileCompression`, pinned to the opened file's `(dev, ino)` so header, metadata and all later
 * range reads address the exact same inode.
 */
export async function openVerifiedArchive(
  path: string,
  tileCompression: Compression,
  cachedDirectories: number,
  expectedYear?: string,
): Promise<VerifiedArchive> {
  const probe = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK)
  let identity: FileIdentity
  try {
    const stat = await probe.stat()
    if (!stat.isFile()) throw new Error(`${path} is not a regular file`)
    identity = { dev: stat.dev, ino: stat.ino }
  } finally {
    await probe.close()
  }
  const source = new FileRangeSource(path, identity)
  const pmtiles = new PMTiles(
    source,
    new SharedPromiseCache(cachedDirectories, true, inflateDirectory),
    async tile => tile,
  )
  const header = await pmtiles.getHeader()
  if (header.tileCompression !== tileCompression) {
    throw new Error(`tile compression ${header.tileCompression} contradicts serving as ${tileCompression}`)
  }
  if (expectedYear !== undefined) {
    if (header.jsonMetadataLength === 0) {
      throw new Error(`${path} lacks metadata year`)
    }
    const raw = await source.getBytes(header.jsonMetadataOffset, header.jsonMetadataLength)
    const inflated = await inflateDirectory(raw.data, header.internalCompression)
    const metadata = JSON.parse(new TextDecoder('utf-8').decode(inflated)) as { year?: unknown }
    if (metadata?.year !== expectedYear) {
      throw new Error(`${path} is for year ${String(metadata?.year)}, not ${expectedYear}`)
    }
  }
  const verifyIdentity = async () => {
    const stat = await lstat(path)
    if (!stat.isFile() || stat.dev !== identity.dev || stat.ino !== identity.ino) {
      throw new ArchiveChangedError(`${path} was replaced after validation`)
    }
  }
  return { pmtiles, verifyIdentity }
}

export async function openArchiveServedAsStored(
  path: string,
  tileCompression: Compression,
  cachedDirectories: number,
  expectedYear?: string,
): Promise<PMTiles> {
  return (await openVerifiedArchive(path, tileCompression, cachedDirectories, expectedYear)).pmtiles
}
