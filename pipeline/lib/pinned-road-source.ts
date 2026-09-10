/** Read an already-retained immutable national road source with byte identity. */

import { createHash } from 'node:crypto'
import { existsSync, readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import type { RoadLoaderArguments } from './road-loader-cli.js'

export function readPinnedRoadSource(
  options: RoadLoaderArguments,
  relativePath: string,
  expectedSha256: string,
): Buffer {
  const path = resolve(options.enrichmentDirectory, relativePath)
  if (options.forceDownload) {
    throw new Error(`${relativePath}: this immutable release source has no download fallback`)
  }
  if (!existsSync(path)) throw new Error(`national road source missing: ${path}`)
  const bytes = readFileSync(path)
  const digest = createHash('sha256').update(bytes).digest('hex')
  if (digest !== expectedSha256) {
    throw new Error(`${relativePath}: SHA-256 ${digest} does not match the admitted release source`)
  }
  return bytes
}
