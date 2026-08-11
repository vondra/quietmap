//! Collision-proof, human-readable log-directory allocation for chain runs.

import { randomBytes } from 'node:crypto'
import { mkdirSync } from 'node:fs'
import { basename, resolve } from 'node:path'

export interface ChainRunDirectory {
  runId: string
  logDir: string
}

/** Allocate a unique run directory while keeping the UTC start second visible
 *  in its name. Exclusive `mkdir` creates the leaf atomically, so concurrent
 *  starts can never share plans, logs, gates, or status files. */
export function createChainRunDirectory(repoRoot: string, startedAt = new Date()): ChainRunDirectory {
  const chainLogsDir = resolve(repoRoot, 'logs', 'chain')
  mkdirSync(chainLogsDir, { recursive: true })
  const readablePrefix = startedAt.toISOString().replace(/[:T]/g, '-').slice(0, 19) + '-'
  for (let attempt = 0; attempt < 128; attempt++) {
    const logDir = resolve(chainLogsDir, readablePrefix + randomBytes(3).toString('hex'))
    try {
      mkdirSync(logDir)
      return { runId: basename(logDir), logDir }
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'EEXIST') throw error
    }
  }
  throw new Error(`could not allocate a unique chain run directory under ${chainLogsDir}`)
}
