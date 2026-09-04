import { existsSync, readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { isDeepStrictEqual } from 'node:util'

function installHint(serverRoot) {
  return `server dependencies do not match package-lock.json; run "cd ${serverRoot} && npm ci"`
}

function readLock(path, hint, contents) {
  try {
    return JSON.parse(contents ?? readFileSync(path, 'utf8'))
  } catch (error) {
    throw new Error(`${hint} (${path}: ${error instanceof Error ? error.message : String(error)})`)
  }
}

/**
 * npm ci writes node_modules/.package-lock.json from the resolved packages it
 * actually installed on this platform. It intentionally omits the root entry
 * and platform-inapplicable optional packages, so it cannot be byte-compared
 * with package-lock.json. Every installed entry must nevertheless be an exact
 * entry from the root lock, and every direct dependency must be installed.
 */
export function assertInstalledDependenciesMatchLock(
  serverRoot,
  {
    nodeModulesRoot = resolve(serverRoot, 'node_modules'),
    rootLockContents,
  } = {},
) {
  const hint = installHint(serverRoot)
  const rootLockPath = resolve(serverRoot, 'package-lock.json')
  const installedLockPath = resolve(nodeModulesRoot, '.package-lock.json')
  if (!existsSync(installedLockPath)) throw new Error(`${hint} (missing ${installedLockPath})`)

  const rootLock = readLock(rootLockPath, hint, rootLockContents)
  const installedLock = readLock(installedLockPath, hint)
  if (rootLock.lockfileVersion !== installedLock.lockfileVersion
    || !rootLock.packages || typeof rootLock.packages !== 'object'
    || !installedLock.packages || typeof installedLock.packages !== 'object') {
    throw new Error(`${hint} (incompatible npm lock metadata)`)
  }

  for (const [packagePath, installedEntry] of Object.entries(installedLock.packages)) {
    const expectedEntry = rootLock.packages[packagePath]
    if (!expectedEntry || !isDeepStrictEqual(installedEntry, expectedEntry)) {
      throw new Error(`${hint} (stale installed entry: ${packagePath})`)
    }
    if (!packagePath.startsWith('node_modules/')
      || !existsSync(resolve(nodeModulesRoot, packagePath.slice('node_modules/'.length)))) {
      throw new Error(`${hint} (installed lock entry is missing on disk: ${packagePath})`)
    }
  }

  const rootPackage = rootLock.packages['']
  if (!rootPackage || typeof rootPackage !== 'object') {
    throw new Error(`${hint} (package-lock.json has no root package)`)
  }
  const directNames = new Set([
    ...Object.keys(rootPackage.dependencies ?? {}),
    ...Object.keys(rootPackage.devDependencies ?? {}),
  ])
  for (const dependency of directNames) {
    const packagePath = `node_modules/${dependency}`
    const installedEntry = installedLock.packages[packagePath]
    const expectedEntry = rootLock.packages[packagePath]
    if (!installedEntry || !expectedEntry || !isDeepStrictEqual(installedEntry, expectedEntry)) {
      throw new Error(`${hint} (missing or stale direct dependency: ${dependency})`)
    }
  }
}
