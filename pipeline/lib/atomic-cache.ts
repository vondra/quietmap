/** Atomic replacement for cache files that may be built by concurrent processes. */

import { randomUUID } from 'node:crypto'
import { promises as fs, renameSync, rmSync } from 'node:fs'

function privateTemporaryPath(destination: string): string {
  return `${destination}.tmp-${process.pid}-${randomUUID()}`
}

/**
 * Build a complete file beside `destination`, then atomically publish it.
 * Each writer owns a unique temporary path: Node's test runner and parallel
 * enrichers may cold-start the same cache without stealing one another's temp.
 * A hard process kill may orphan that path; never sweep it while a concurrent
 * writer could still own it.
 */
export function replaceCacheFileAtomically(
  destination: string,
  build: (temporaryPath: string) => void,
): void {
  const temporaryPath = privateTemporaryPath(destination)
  try {
    build(temporaryPath)
    renameSync(temporaryPath, destination)
  } finally {
    rmSync(temporaryPath, { force: true })
  }
}

/** Async twin for streamed cache builders; publication waits for completion. */
export async function replaceCacheFileAtomicallyAsync(
  destination: string,
  build: (temporaryPath: string) => Promise<void>,
): Promise<void> {
  const temporaryPath = privateTemporaryPath(destination)
  try {
    await build(temporaryPath)
    await fs.rename(temporaryPath, destination)
  } finally {
    await fs.rm(temporaryPath, { force: true })
  }
}
