import { fileURLToPath } from 'node:url'
import { activatePreparedRelease, rollbackRelease } from './release-layout.mjs'
import { ensureReleaseLock } from './release-lock.mjs'

ensureReleaseLock(fileURLToPath(import.meta.url))

if (process.argv.includes('--rollback')) {
  console.log(`rolled back server release to ${rollbackRelease()}`)
} else {
  const result = activatePreparedRelease()
  console.log(`activated server release ${result.active}`)
}
