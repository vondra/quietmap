// Reproducible immutable server release: compile into a clean staging tree,
// add non-TS assets, validate syntax, then publish through a release symlink.
// Old Node processes resolve the symlink to their own immutable generation,
// so a lazy Worker can never mix old supervisor JS with a new worker asset.
import {
  constants as fsConstants,
  cpSync,
  existsSync,
  lstatSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  readlinkSync,
  renameSync,
  rmSync,
  statSync,
  symlinkSync,
} from 'node:fs'
import { spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { dirname, relative, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import {
  prepareRelease,
  pruneUnusedReleases,
  releaseRoot,
  serverRoot,
} from './release-layout.mjs'
import { ensureReleaseLock } from './release-lock.mjs'
import { assertInstalledDependenciesMatchLock } from './dependency-lock.mjs'

ensureReleaseLock(fileURLToPath(import.meta.url))
const requireNative = process.argv.includes('--require-native')
const frontendArg = process.argv.indexOf('--frontend-dir')
const frontendSource = frontendArg >= 0 && process.argv[frontendArg + 1]
  ? resolve(serverRoot, process.argv[frontendArg + 1])
  : null
if (frontendArg >= 0 && !frontendSource) throw new Error('--frontend-dir requires a path')
const repoRoot = resolve(serverRoot, '..')

/** Detect an editor/agent changing files while tsc and the worker copy run. */
function sourceInputFingerprint() {
  const listed = spawnSync('git', [
    'ls-files', '--cached', '--others', '--exclude-standard', '-z', '--',
    'server/src', 'server/package.json', 'server/package-lock.json',
    'server/tsconfig.json', 'server/tsconfig.build.json',
  ], { cwd: repoRoot, encoding: 'utf8' })
  if (listed.status !== 0) throw new Error(`cannot list server build inputs: ${listed.stderr.trim()}`)
  const hash = createHash('sha256')
  for (const path of [...new Set(listed.stdout.split('\0').filter(Boolean))].sort()) {
    hash.update(path).update('\0')
    try { hash.update(readFileSync(resolve(repoRoot, path))) }
    catch (error) {
      if (error?.code !== 'ENOENT') throw error
      hash.update('<missing>')
    }
    hash.update('\0')
  }
  return hash.digest('hex')
}

function fileFingerprint(path) {
  return existsSync(path) ? createHash('sha256').update(readFileSync(path)).digest('hex') : null
}

function treeFingerprint(root) {
  if (!existsSync(root)) return null
  const hash = createHash('sha256')
  const walk = (dir, prefix = '') => {
    for (const entry of readdirSync(dir, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
      const relativePath = prefix ? `${prefix}/${entry.name}` : entry.name
      const path = resolve(dir, entry.name)
      const stat = lstatSync(path)
      hash.update(relativePath).update('\0')
      if (stat.isDirectory()) { hash.update('dir\0'); walk(path, relativePath) }
      else if (stat.isSymbolicLink()) hash.update('link\0').update(readlinkSync(path)).update('\0')
      else if (stat.isFile()) hash.update('file\0').update(readFileSync(path)).update('\0')
      else throw new Error(`unsupported build input: ${path}`)
    }
  }
  walk(root)
  return hash.digest('hex')
}

const nativeSource = resolve(serverRoot, '..', 'engine/source-reader/target/release/libsource_reader.so')
const sourceInputsBefore = sourceInputFingerprint()
const dependencyLock = readFileSync(resolve(serverRoot, 'package-lock.json'))
// A release is named after package-lock.json, so copying modules installed
// from a different lock would permanently mislabel and cache stale code.
// Validate against the exact bytes hashed below; the final source fingerprint
// also refuses publication if an editor changes the lock during this build.
const dependencyLockOptions = { rootLockContents: dependencyLock.toString('utf8') }
assertInstalledDependenciesMatchLock(serverRoot, dependencyLockOptions)
const frontendInputBefore = frontendSource ? treeFingerprint(frontendSource) : null
const nativeInputBefore = fileFingerprint(nativeSource)
mkdirSync(releaseRoot, { recursive: true })
const dependencyRoot = resolve(serverRoot, '..', '.server-deps')
mkdirSync(dependencyRoot, { recursive: true })
const dependencyHash = createHash('sha256')
  .update(dependencyLock)
  .update(`\0${process.platform}\0${process.arch}\0${process.versions.modules}`)
  .digest('hex')
  .slice(0, 20)
const dependencySnapshot = resolve(dependencyRoot, `deps-${dependencyHash}`)
const dependencyModules = resolve(dependencySnapshot, 'node_modules')
let dependencySnapshotValid = false
if (existsSync(resolve(dependencyModules, 'fastify/package.json'))) {
  try {
    assertInstalledDependenciesMatchLock(serverRoot, {
      ...dependencyLockOptions,
      nodeModulesRoot: dependencyModules,
    })
    dependencySnapshotValid = true
  } catch {
    // A build predating the lock check may have poisoned this cache key.
    // The live node_modules was validated above, so rebuilding is local-only.
    rmSync(dependencySnapshot, { recursive: true, force: true })
  }
}
if (!dependencySnapshotValid) {
  const dependencyStage = resolve(dependencyRoot, `.stage-deps-${dependencyHash}-${process.pid}`)
  try {
    rmSync(dependencySnapshot, { recursive: true, force: true })
    rmSync(dependencyStage, { recursive: true, force: true })
    mkdirSync(dependencyStage, { recursive: true })
    cpSync(resolve(serverRoot, 'node_modules'), resolve(dependencyStage, 'node_modules'), {
      recursive: true,
      mode: fsConstants.COPYFILE_FICLONE,
      verbatimSymlinks: true,
    })
    renameSync(dependencyStage, dependencySnapshot)
    dependencySnapshotValid = true
  } finally {
    rmSync(dependencyStage, { recursive: true, force: true })
  }
}
if (!statSync(dependencyModules).isDirectory()
  || !existsSync(resolve(dependencyModules, 'fastify/package.json'))) {
  throw new Error(`invalid server dependency snapshot: ${dependencySnapshot}`)
}
assertInstalledDependenciesMatchLock(serverRoot, {
  ...dependencyLockOptions,
  nodeModulesRoot: dependencyModules,
})
const releaseName = `release-${new Date().toISOString().replace(/[^0-9TZ]/g, '')}-${process.pid}`
const stage = resolve(releaseRoot, `.stage-${releaseName}`)
const release = resolve(releaseRoot, releaseName)
let published = false
let prepared = false
process.once('exit', () => {
  rmSync(stage, { recursive: true, force: true })
  if (published && !prepared) rmSync(release, { recursive: true, force: true })
})
rmSync(stage, { recursive: true, force: true })
mkdirSync(stage, { recursive: true })
symlinkSync(relative(stage, dependencyModules), resolve(stage, 'node_modules'), 'dir')
cpSync(resolve(serverRoot, 'package.json'), resolve(stage, 'package.json'))

const tsc = resolve(serverRoot, 'node_modules/typescript/bin/tsc')
const compiled = spawnSync(process.execPath, [
  tsc,
  '--project',
  'tsconfig.build.json',
  '--outDir',
  stage,
  '--noEmitOnError',
], {
  cwd: serverRoot,
  stdio: 'inherit',
})
if (compiled.status !== 0) {
  rmSync(stage, { recursive: true, force: true })
  process.exit(compiled.status ?? 1)
}

const runtimeAssets = [
  ['src/generation-contract.mjs', 'generation-contract.mjs'],
  ['src/workers/noise-onfly-worker.mjs', 'workers/noise-onfly-worker.mjs'],
]
for (const [sourceRelative, destinationRelative] of runtimeAssets) {
  const source = resolve(serverRoot, sourceRelative)
  const destination = resolve(stage, destinationRelative)
  if (!existsSync(source)) throw new Error(`missing runtime asset: ${sourceRelative}`)
  mkdirSync(dirname(destination), { recursive: true })
  cpSync(source, destination)
}

if (frontendSource) {
  if (!existsSync(resolve(frontendSource, 'index.html'))) {
    throw new Error(`prepared frontend has no index.html: ${frontendSource}`)
  }
  cpSync(frontendSource, resolve(stage, 'frontend'), {
    recursive: true,
    mode: fsConstants.COPYFILE_FICLONE,
  })
}

if (existsSync(nativeSource) && statSync(nativeSource).isFile() && statSync(nativeSource).size > 0) {
  const nativeDestination = resolve(stage, 'native/libsource_reader.so')
  mkdirSync(dirname(nativeDestination), { recursive: true })
  cpSync(nativeSource, nativeDestination)
} else if (requireNative) {
  rmSync(stage, { recursive: true, force: true })
  throw new Error(`missing required native addon: ${nativeSource}`)
}

for (const relative of ['server.js', ...runtimeAssets.map(([, destination]) => destination)]) {
  const checked = spawnSync(process.execPath, ['--check', resolve(stage, relative)], {
    cwd: serverRoot,
    stdio: 'inherit',
  })
  if (checked.status !== 0) {
    rmSync(stage, { recursive: true, force: true })
    process.exit(checked.status ?? 1)
  }
}

// Syntax checks do not resolve imports. Load a staged module that crosses the
// emitted TypeScript -> copied .mjs boundary before publishing the release.
const imported = spawnSync(process.execPath, [
  '--input-type=module',
  '--eval',
  `await import(${JSON.stringify(pathToFileURL(resolve(stage, 'runtime-readiness.js')).href)})`,
], {
  cwd: stage,
  stdio: 'inherit',
})
if (imported.status !== 0) {
  rmSync(stage, { recursive: true, force: true })
  process.exit(imported.status ?? 1)
}

if (sourceInputFingerprint() !== sourceInputsBefore) {
  throw new Error('server build inputs changed during compilation; refusing to publish a mixed release')
}
if (frontendSource) {
  const frontendInputAfter = treeFingerprint(frontendSource)
  const frontendCopy = treeFingerprint(resolve(stage, 'frontend'))
  if (frontendInputAfter !== frontendInputBefore || frontendCopy !== frontendInputBefore) {
    throw new Error('frontend build changed while being copied; refusing to publish a mixed release')
  }
}
const nativeInputAfter = fileFingerprint(nativeSource)
const nativeCopy = fileFingerprint(resolve(stage, 'native/libsource_reader.so'))
if (nativeInputAfter !== nativeInputBefore || nativeCopy !== nativeInputBefore) {
  throw new Error('native addon changed while being copied; refusing to publish a mixed release')
}

renameSync(stage, release)
published = true
prepareRelease(release)
prepared = true
pruneUnusedReleases()
console.log(`prepared immutable server release ${release}`)
