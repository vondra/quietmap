/** Process-wide singleton lock backed by SQLite's crash-safe file locking. */
import { DatabaseSync } from 'node:sqlite'

export interface SqliteRunLock {
  release(): void
}

/** Acquire an exclusive lock without waiting, or return null when another owner holds it. */
export function tryAcquireSqliteRunLock(lockPath: string): SqliteRunLock | null {
  const lockDatabase = new DatabaseSync(lockPath)
  try {
    lockDatabase.exec('PRAGMA busy_timeout = 0; BEGIN EXCLUSIVE')
  } catch (error) {
    const sqliteErrorCode = (error as { errcode?: number }).errcode
    lockDatabase.close()
    if (sqliteErrorCode !== undefined && (sqliteErrorCode & 0xff) === 5) return null
    throw error
  }

  let released = false
  return {
    release(): void {
      if (released) return
      released = true
      try {
        lockDatabase.exec('ROLLBACK')
      } finally {
        lockDatabase.close()
      }
    },
  }
}
