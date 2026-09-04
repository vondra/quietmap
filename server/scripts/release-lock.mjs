import { spawnSync } from 'node:child_process'
import { writeSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const serverRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')

/** Re-exec a release-mutating/checking script under the checkout-wide lock. */
export function ensureReleaseLock(scriptPath) {
  if (process.env.QM_RELEASE_LOCK_HELD === '1') return
  const locked = spawnSync('flock', [
    '-n',
    '-E',
    '75',
    resolve(serverRoot, '..', '.release.lock'),
    process.execPath,
    scriptPath,
    ...process.argv.slice(2),
  ], {
    cwd: process.cwd(),
    env: { ...process.env, QM_RELEASE_LOCK_HELD: '1' },
    stdio: 'inherit',
  })
  if (locked.status === 75) writeSync(2, 'another build/deploy owns this checkout release lock\n')
  process.exit(locked.status ?? 1)
}
