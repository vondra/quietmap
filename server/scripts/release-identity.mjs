/** release.json inside each immutable server release: the product commit, dirty flag and packaged bytes it serves. */
import { spawnSync } from 'node:child_process'
import { writeFileSync } from 'node:fs'
import { resolve } from 'node:path'

export const releaseIdentityFileName = 'release.json'

/** The checked-out product commit and whether the working tree differs from it. */
export function productCodeIdentity(repoRoot) {
  const git = (...args) => {
    const result = spawnSync('git', args, { cwd: repoRoot, encoding: 'utf8' })
    if (result.status !== 0) throw new Error(`git ${args[0]} failed: ${result.stderr.trim()}`)
    return result.stdout
  }
  return {
    product_commit: git('rev-parse', 'HEAD').trim(),
    product_dirty: git('status', '--porcelain', '--untracked-files=normal').trim() !== '',
  }
}

/** Written once into the staged release before it is renamed into place; never overwritten. */
export function writeReleaseIdentity(stage, identity) {
  writeFileSync(resolve(stage, releaseIdentityFileName), `${JSON.stringify(identity, null, 1)}\n`, { flag: 'wx' })
}
