/** Atomic same-directory replacement for reusable enrichment cache files. */

import {
  existsSync, mkdirSync, renameSync, unlinkSync, writeFileSync,
} from 'node:fs'
import { dirname } from 'node:path'

export function writeCacheAtomically(path: string, bytes: string | Buffer): void {
  mkdirSync(dirname(path), { recursive: true })
  const temporaryPath = `${path}.tmp-${process.pid}`
  try {
    writeFileSync(temporaryPath, bytes)
    renameSync(temporaryPath, path)
  } finally {
    if (existsSync(temporaryPath)) unlinkSync(temporaryPath)
  }
}
