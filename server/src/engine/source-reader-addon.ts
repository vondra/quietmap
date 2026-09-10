import {
  copyFileSync,
  existsSync,
  lstatSync,
  readdirSync,
  renameSync,
  rmSync,
  statSync,
  utimesSync,
} from 'node:fs'
import { createRequire } from 'node:module'
import { dirname, resolve } from 'node:path'

/**
 * Prepare the one process-stable addon path before any Worker is spawned.
 * This runs on Node's main thread, so simultaneous pool dispatches cannot
 * unlink or require a half-copied native library from competing workers.
 */
export function prepareSourceReaderAddon(sourceReaderPath: string): string {
  const sourceReaderDir = dirname(sourceReaderPath)
  const nodePath = resolve(sourceReaderDir, 'libsource_reader.worker-shared.node')
  const source = statSync(sourceReaderPath)
  if (!source.isFile() || source.size === 0) {
    throw new Error(`source-reader addon is not a non-empty regular file: ${sourceReaderPath}`)
  }

  // Sweep obsolete per-thread/per-slot paths. Distinct dlopen paths consume
  // glibc's finite static-TLS surplus even after a Worker is terminated.
  for (const entry of readdirSync(sourceReaderDir)) {
    if (/^libsource_reader\.worker-(?:tid-|slot-)?\d+\.node$/.test(entry)) {
      rmSync(resolve(sourceReaderDir, entry), { force: true })
    }
  }

  let fresh = false
  if (existsSync(nodePath)) {
    const linkInfo = lstatSync(nodePath)
    if (!linkInfo.isSymbolicLink()) {
      const destination = statSync(nodePath)
      fresh = destination.isFile()
        && !(source.dev === destination.dev && source.ino === destination.ino)
        && source.size === destination.size
        && Math.abs(source.mtimeMs - destination.mtimeMs) <= 2
    }
  }

  if (!fresh) {
    const temporary = `${nodePath}.tmp-${process.pid}-${Date.now()}`
    try {
      copyFileSync(sourceReaderPath, temporary)
      utimesSync(temporary, source.atime, source.mtime)
      // POSIX rename replaces the old inode atomically. Workers that already
      // mapped it keep their inode; a new process sees the complete new copy.
      renameSync(temporary, nodePath)
    } finally {
      rmSync(temporary, { force: true })
    }
  }

  return nodePath
}

let pinnedOnMainThread = false

/**
 * Hold the addon in the main thread for the process lifetime. Workers load
 * the same path; when the last worker holding it exited, Node dlclose'd the
 * library while its Rust threads were alive and the next spawn's re-map
 * crashed the server (SIGSEGV twice on 2026-09-06). Separate from
 * prepareSourceReaderAddon: that one also serves plain-file tests.
 */
export function pinSourceReaderAddonOnMainThread(nodePath: string): void {
  if (pinnedOnMainThread) return
  createRequire(import.meta.url)(nodePath)
  pinnedOnMainThread = true
}
