import assert from 'node:assert/strict'
import { spawn, spawnSync, type ChildProcess } from 'node:child_process'
import {
  access,
  copyFile,
  lstat,
  mkdir,
  mkdtemp,
  readFile,
  realpath,
  rm,
  symlink,
  utimes,
  writeFile,
} from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { basename, dirname, join, relative, resolve } from 'node:path'
import test, { type TestContext } from 'node:test'
import { pathToFileURL } from 'node:url'

interface ReleaseLayoutModule {
  activatePreparedRelease(): {
    active: string
    previous: string | null
  }
  prepareRelease(releasePath: string): void
  pruneUnusedReleases(): void
  releaseRoot: string
  resolveNamedRelease(name: string): string
  rollbackRelease(): string
  serverRoot: string
}

interface ReleaseFixture {
  createRelease(name: string, marker?: string): Promise<string>
  dependencySnapshot: string
  dist: string
  next: string
  previous: string
  releaseRoot: string
  root: string
  serverRoot: string
  subject: ReleaseLayoutModule
}

const productionScripts = resolve(import.meta.dirname, '..', 'scripts')

async function exists(path: string): Promise<boolean> {
  try {
    await access(path)
    return true
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return false
    throw error
  }
}

async function linkTo(target: string, linkPath: string): Promise<void> {
  await symlink(relative(dirname(linkPath), target), linkPath, 'dir')
}

async function releaseFixture(
  t: TestContext,
  { injectActivationFailure = false }: { injectActivationFailure?: boolean } = {},
): Promise<ReleaseFixture> {
  const root = await mkdtemp(join(tmpdir(), '0db-release-layout-'))
  t.after(async () => rm(root, { recursive: true, force: true }))
  const serverRoot = join(root, 'server')
  const scripts = join(serverRoot, 'scripts')
  const releaseRoot = join(root, '.server-dist')
  const dependencySnapshot = join(root, '.server-deps', 'deps-fixture', 'node_modules')
  await mkdir(join(serverRoot, 'node_modules'), { recursive: true })
  await mkdir(scripts, { recursive: true })
  await mkdir(releaseRoot, { recursive: true })
  await mkdir(dependencySnapshot, { recursive: true })

  const production = await readFile(join(productionScripts, 'release-layout.mjs'), 'utf8')
  let fixtureSource = production
  if (injectActivationFailure) {
    const needle = '    renameSync(nextPath, distPath)'
    assert.equal(
      production.split(needle).length - 1,
      1,
      'failure fixture must instrument exactly one active-pointer rename',
    )
    fixtureSource = production.replace(
      needle,
      [
        '    if (globalThis.__QM_TEST_FAIL_ACTIVATION__ === true) {',
        "      throw new Error('injected activation failure')",
        '    }',
        needle,
      ].join('\n'),
    )
  }
  const fixtureModule = join(scripts, 'release-layout.mjs')
  await writeFile(fixtureModule, fixtureSource)
  const subject = await import(pathToFileURL(fixtureModule).href) as ReleaseLayoutModule

  return {
    root,
    serverRoot,
    releaseRoot,
    dependencySnapshot,
    dist: join(serverRoot, 'dist'),
    next: join(serverRoot, 'dist.next'),
    previous: join(serverRoot, 'dist.previous'),
    subject,
    async createRelease(name: string, marker = name) {
      const path = join(releaseRoot, name)
      await mkdir(path, { recursive: true })
      await writeFile(join(path, 'server.js'), marker)
      await linkTo(dependencySnapshot, join(path, 'node_modules'))
      return path
    },
  }
}

test('fresh releases prepare, activate, and roll back through immutable links', async (t) => {
  const fixture = await releaseFixture(t)
  const first = await fixture.createRelease('release-first')
  fixture.subject.prepareRelease(first)
  assert.equal(await realpath(fixture.next), first)
  assert.equal(await realpath(join(first, 'node_modules')), fixture.dependencySnapshot)
  assert.equal((await lstat(join(first, 'node_modules'))).isSymbolicLink(), true)

  const firstActivation = fixture.subject.activatePreparedRelease()
  assert.equal(firstActivation.active, first)
  assert.equal(firstActivation.previous, null)
  assert.equal(await realpath(fixture.dist), first)
  assert.equal(await exists(fixture.next), false)
  assert.equal(await exists(fixture.previous), false)

  const second = await fixture.createRelease('release-second')
  fixture.subject.prepareRelease(second)
  const secondActivation = fixture.subject.activatePreparedRelease()
  assert.equal(secondActivation.active, second)
  assert.equal(await realpath(fixture.dist), second)
  assert.equal(await realpath(fixture.previous), first)

  assert.equal(fixture.subject.rollbackRelease(), first)
  assert.equal(await realpath(fixture.dist), first)
  assert.equal(await exists(fixture.previous), false)
})

test('outside and dangling release links fail closed before garbage collection', async (t) => {
  const fixture = await releaseFixture(t)
  const unreferenced = await fixture.createRelease('release-must-survive')
  const outside = join(fixture.root, 'outside-release')
  await mkdir(outside)
  await linkTo(outside, fixture.next)

  assert.throws(
    () => fixture.subject.resolveNamedRelease('dist.next'),
    /points outside/,
  )
  assert.throws(() => fixture.subject.pruneUnusedReleases(), /points outside/)
  assert.equal(await exists(unreferenced), true, 'failed validation must not prune anything')
  assert.throws(() => fixture.subject.prepareRelease(outside), /outside/)

  await rm(fixture.next)
  await linkTo(join(fixture.releaseRoot, 'release-missing'), fixture.next)
  assert.throws(
    () => fixture.subject.pruneUnusedReleases(),
    (error: unknown) => (error as NodeJS.ErrnoException).code === 'ENOENT',
  )
  assert.equal(await exists(unreferenced), true, 'dangling-link failure must remain fail closed')
})

test('garbage collection preserves referenced releases and removes only safe stale entries', async (t) => {
  const fixture = await releaseFixture(t)
  const active = await fixture.createRelease('release-active')
  const next = await fixture.createRelease('release-next')
  const previous = await fixture.createRelease('release-previous')
  const orphan = await fixture.createRelease('release-orphan')
  const secondOrphan = await fixture.createRelease('release-second-orphan')
  const staleStage = await fixture.createRelease('.stage-stale')
  const freshStage = await fixture.createRelease('.stage-fresh')
  const unrelated = await fixture.createRelease('operator-notes')
  await linkTo(active, fixture.dist)
  await linkTo(next, fixture.next)
  await linkTo(previous, fixture.previous)
  const twoHoursAgo = new Date(Date.now() - 2 * 60 * 60 * 1_000)
  await utimes(staleStage, twoHoursAgo, twoHoursAgo)

  fixture.subject.pruneUnusedReleases()

  for (const kept of [active, next, previous, freshStage, unrelated]) {
    assert.equal(await exists(kept), true, `${basename(kept)} should be retained`)
  }
  for (const pruned of [orphan, secondOrphan, staleStage]) {
    assert.equal(await exists(pruned), false, `${basename(pruned)} should be pruned`)
  }
})

test('failed activation restores the original previous pointer', async (t) => {
  const fixture = await releaseFixture(t, { injectActivationFailure: true })
  const active = await fixture.createRelease('release-active')
  const originalPrevious = await fixture.createRelease('release-original-previous')
  const prepared = await fixture.createRelease('release-prepared')
  await linkTo(active, fixture.dist)
  await linkTo(originalPrevious, fixture.previous)
  await linkTo(prepared, fixture.next)

  const injectionState = globalThis as typeof globalThis & {
    __QM_TEST_FAIL_ACTIVATION__?: boolean
  }
  injectionState.__QM_TEST_FAIL_ACTIVATION__ = true
  try {
    assert.throws(
      () => fixture.subject.activatePreparedRelease(),
      /injected activation failure/,
    )
  } finally {
    delete injectionState.__QM_TEST_FAIL_ACTIVATION__
  }

  assert.equal(await realpath(fixture.dist), active)
  assert.equal(await realpath(fixture.previous), originalPrevious)
  assert.equal(await realpath(fixture.next), prepared)
})

async function waitForLocked(holder: ChildProcess): Promise<void> {
  await new Promise<void>((resolveReady, reject) => {
    const timeout = setTimeout(() => reject(new Error('timed out waiting for fixture lock')), 5_000)
    holder.once('error', (error) => {
      clearTimeout(timeout)
      reject(error)
    })
    holder.once('exit', (code, signal) => {
      clearTimeout(timeout)
      reject(new Error(`lock holder exited early code=${code} signal=${signal}`))
    })
    holder.stdout?.setEncoding('utf8')
    holder.stdout?.on('data', (chunk: string) => {
      if (!chunk.includes('fixture-locked')) return
      clearTimeout(timeout)
      resolveReady()
    })
  })
}

test('release lock re-execs once and rejects a concurrent contender', async (t) => {
  const root = await mkdtemp(join(tmpdir(), '0db-release-lock-'))
  t.after(async () => rm(root, { recursive: true, force: true }))
  const scripts = join(root, 'server', 'scripts')
  await mkdir(scripts, { recursive: true })
  await copyFile(join(productionScripts, 'release-lock.mjs'), join(scripts, 'release-lock.mjs'))
  const probe = join(scripts, 'probe.mjs')
  const probeLog = join(root, 'probe.log')
  await writeFile(probe, [
    "import { appendFileSync } from 'node:fs'",
    "import { fileURLToPath } from 'node:url'",
    "import { ensureReleaseLock } from './release-lock.mjs'",
    'ensureReleaseLock(fileURLToPath(import.meta.url))',
    "appendFileSync(process.env.PROBE_LOG, `${process.env.QM_RELEASE_LOCK_HELD}\\n`)",
  ].join('\n'))
  const probeEnv = {
    ...process.env,
    PROBE_LOG: probeLog,
    QM_RELEASE_LOCK_HELD: '',
  }

  const uncontended = spawnSync(process.execPath, [probe], {
    env: probeEnv,
    encoding: 'utf8',
  })
  assert.equal(uncontended.status, 0, uncontended.stderr)
  assert.equal(await readFile(probeLog, 'utf8'), '1\n', 'only the locked child runs the probe')
  await writeFile(probeLog, '')

  const lockPath = join(root, '.release.lock')
  const holder = spawn('flock', [
    '-n',
    '-F',
    lockPath,
    '/bin/sh',
    '-c',
    "printf 'fixture-locked\\n'; exec sleep 30",
  ], { stdio: ['ignore', 'pipe', 'pipe'] })
  t.after(() => {
    if (holder.exitCode === null && holder.signalCode === null) holder.kill('SIGKILL')
  })
  await waitForLocked(holder)

  const contended = spawnSync(process.execPath, [probe], {
    env: probeEnv,
    encoding: 'utf8',
  })
  assert.equal(contended.status, 75)
  assert.equal(await readFile(probeLog, 'utf8'), '', 'contended probe must not run')
  holder.kill('SIGTERM')
})
