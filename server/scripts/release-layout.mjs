import {
  existsSync,
  lstatSync,
  mkdirSync,
  readdirSync,
  readlinkSync,
  realpathSync,
  renameSync,
  rmSync,
  statSync,
  symlinkSync,
} from 'node:fs'
import { dirname, isAbsolute, relative, resolve, sep } from 'node:path'
import { fileURLToPath } from 'node:url'

export const serverRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')
export const releaseRoot = resolve(serverRoot, '..', '.server-dist')
const dependencyRoot = resolve(serverRoot, '..', '.server-deps')
const distPath = resolve(serverRoot, 'dist')
const nextPath = resolve(serverRoot, 'dist.next')
const previousPath = resolve(serverRoot, 'dist.previous')

function isInsideReleaseRoot(candidate) {
  const pathFromRoot = relative(realpathSync(releaseRoot), candidate)
  return pathFromRoot !== ''
    && pathFromRoot !== '..'
    && !pathFromRoot.startsWith(`..${sep}`)
    && !isAbsolute(pathFromRoot)
}

function validatedReleaseLink(linkPath) {
  const info = lstatSync(linkPath)
  if (!info.isSymbolicLink()) throw new Error(`${linkPath} is not a release symlink`)
  const real = realpathSync(linkPath)
  if (!isInsideReleaseRoot(real) || !statSync(real).isDirectory()) {
    throw new Error(`${linkPath} points outside ${releaseRoot}`)
  }
  return { raw: readlinkSync(linkPath), real }
}

function replaceSymlink(linkPath, target) {
  const temporary = `${linkPath}.tmp-${process.pid}-${Date.now()}`
  try {
    symlinkSync(target, temporary, 'dir')
    renameSync(temporary, linkPath)
  } finally {
    rmSync(temporary, { force: true })
  }
}

function ensureRuntimeDependencies(releasePath) {
  const dependencyPath = resolve(releasePath, 'node_modules')
  if (!existsSync(dependencyPath)) {
    throw new Error(`${dependencyPath} is missing from an immutable release`)
  }
  if (!lstatSync(dependencyPath).isSymbolicLink()) {
    throw new Error(`${dependencyPath} is not a dependency snapshot symlink`)
  }
  const actual = realpathSync(dependencyPath)
  const pathFromRoot = relative(realpathSync(dependencyRoot), actual)
  if (pathFromRoot === ''
      || pathFromRoot === '..'
      || pathFromRoot.startsWith(`..${sep}`)
      || isAbsolute(pathFromRoot)) {
    throw new Error(`${dependencyPath} points outside the dependency snapshots`)
  }
}

export function prepareRelease(releasePath) {
  mkdirSync(releaseRoot, { recursive: true })
  const real = realpathSync(releasePath)
  if (!isInsideReleaseRoot(real) || !statSync(real).isDirectory()) {
    throw new Error(`refusing to prepare release outside ${releaseRoot}`)
  }
  ensureRuntimeDependencies(real)
  replaceSymlink(nextPath, relative(serverRoot, real))
}

export function resolveNamedRelease(name) {
  const linkPath = name === 'dist'
    ? distPath
    : name === 'dist.next'
      ? nextPath
      : null
  if (!linkPath) throw new Error(`invalid server release link ${JSON.stringify(name)}`)
  const release = validatedReleaseLink(linkPath)
  ensureRuntimeDependencies(release.real)
  return release.real
}

export function pruneUnusedReleases() {
  const keep = new Set()
  for (const linkPath of [distPath, nextPath, previousPath]) {
    let linkInfo
    try {
      linkInfo = lstatSync(linkPath)
    } catch (error) {
      if (error?.code === 'ENOENT') continue
      throw error
    }
    // An existing but corrupt/unreadable link aborts pruning. Treating it as
    // absent could recursively delete the generation a live process uses.
    keep.add(validatedReleaseLink(linkPath).real)
  }
  const staleStageBefore = Date.now() - 60 * 60 * 1_000
  for (const entry of readdirSync(releaseRoot, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue
    const candidate = resolve(releaseRoot, entry.name)
    if (entry.name.startsWith('release-') && !keep.has(candidate)) {
      rmSync(candidate, { recursive: true, force: true })
    } else if (entry.name.startsWith('.stage-') && statSync(candidate).mtimeMs < staleStageBefore) {
      rmSync(candidate, { recursive: true, force: true })
    }
  }
}

export function activatePreparedRelease() {
  const next = validatedReleaseLink(nextPath)
  ensureRuntimeDependencies(next.real)
  let previousTarget = null
  let originalPrevious = null

  try {
    lstatSync(previousPath)
    originalPrevious = validatedReleaseLink(previousPath).raw
  } catch (error) {
    if (error?.code !== 'ENOENT') throw error
  }

  try {
    if (existsSync(distPath)) {
      const current = lstatSync(distPath)
      if (current.isSymbolicLink()) {
        const currentRelease = validatedReleaseLink(distPath)
        ensureRuntimeDependencies(currentRelease.real)
        previousTarget = currentRelease.raw
      } else {
        throw new Error('server/dist is not a release symlink')
      }
    }

    if (previousTarget) replaceSymlink(previousPath, previousTarget)
    else rmSync(previousPath, { force: true })
    renameSync(nextPath, distPath)
  } catch (error) {
    try {
      if (originalPrevious) replaceSymlink(previousPath, originalPrevious)
      else rmSync(previousPath, { force: true })
    } catch (recoveryError) {
      throw new AggregateError([error, recoveryError], 'release activation and pointer recovery both failed')
    }
    throw error
  }

  return { active: next.real, previous: previousTarget }
}

export function rollbackRelease() {
  const previous = validatedReleaseLink(previousPath)
  ensureRuntimeDependencies(previous.real)
  if (!existsSync(distPath) || !lstatSync(distPath).isSymbolicLink()) {
    throw new Error('cannot roll back: server/dist is not an active release symlink')
  }
  renameSync(previousPath, distPath)
  return previous.real
}
