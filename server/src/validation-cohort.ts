/** Stable identity of the code and prepared inputs used by popup validation. */
import { spawn, type ChildProcess } from 'node:child_process'
import { createHash } from 'node:crypto'
import { readdir, readFile, stat } from 'node:fs/promises'
import { relative, resolve } from 'node:path'
import { DATA_YEAR } from './data-year.js'
import { H3R4_DIR, SOURCE_READER_PATH } from './runtime-paths.js'

export type ValidationCohort = {
  schema_version: 1
  cohort_id: string
  /** Public recomputation throttle; runners wait this out before their final check. */
  cache_ttl_ms: number
  data_year: string
  runtime_sha256: string
  prepared_sha256: string
}

export type ValidationCohortProvider = () => Promise<ValidationCohort>

type ValidationCohortOptions = {
  cacheMs?: number
  dataYear?: string
  h3r4Dir?: string
  modelProcessStartedAtMs?: number
  preparedAuxiliaryInputs?: Array<{ label: string; path: string }>
  runtimeIdentityInputs?: Array<{ label: string; path: string }>
  runtimeRoot?: string
  sourceReaderPath?: string
}

type Fingerprint = { sha256: string; newestChangeMs: number }
type CachedCohort =
  | { expiresAt: number; cohort: ValidationCohort }
  | { expiresAt: number; error: unknown }

const RUNTIME_EXTENSIONS = new Set(['.js', '.mjs', '.ts'])
const SKIPPED_RUNTIME_DIRECTORIES = new Set(['frontend', 'native', 'node_modules'])

function extension(path: string): string {
  const dot = path.lastIndexOf('.')
  return dot === -1 ? '' : path.slice(dot)
}

async function runtimeFiles(root: string, directory = root): Promise<string[]> {
  const result: string[] = []
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    if (entry.isDirectory()) {
      if (!SKIPPED_RUNTIME_DIRECTORIES.has(entry.name)) {
        result.push(...await runtimeFiles(root, resolve(directory, entry.name)))
      }
      continue
    }
    if (!entry.isFile() || entry.name.includes('.test.') || !RUNTIME_EXTENSIONS.has(extension(entry.name))) continue
    result.push(resolve(directory, entry.name))
  }
  return result
}

function sameFileIdentity(before: Awaited<ReturnType<typeof stat>>, after: Awaited<ReturnType<typeof stat>>): boolean {
  return before.dev === after.dev && before.ino === after.ino && before.size === after.size
    && before.mtimeMs === after.mtimeMs && before.ctimeMs === after.ctimeMs
}

async function fingerprintRuntime(
  runtimeRoot: string,
  sourceReaderPath: string,
  identityInputs: Array<{ label: string; path: string }>,
): Promise<Fingerprint> {
  const hash = createHash('sha256')
  let newestChangeMs = 0
  const files = (await runtimeFiles(runtimeRoot)).sort((a, b) => a.localeCompare(b))
  files.push(sourceReaderPath)
  for (const path of files) {
    const label = path === sourceReaderPath ? '<native-addon>' : relative(runtimeRoot, path)
    const before = await stat(path)
    const bytes = await readFile(path)
    const after = await stat(path)
    if (!sameFileIdentity(before, after)) throw new Error(`${path} changed while its runtime fingerprint was read`)
    newestChangeMs = Math.max(newestChangeMs, after.mtimeMs, after.ctimeMs)
    hash.update(label).update('\0').update(bytes).update('\0')
  }
  for (const input of identityInputs) {
    const fingerprint = await fingerprintPreparedInput(input.path)
    newestChangeMs = Math.max(newestChangeMs, fingerprint.newestChangeMs)
    hash.update(input.label).update('\0').update(fingerprint.sha256).update('\0')
  }
  return { sha256: hash.digest('hex'), newestChangeMs }
}

/**
 * Hash only immutable file identity metadata, not hundreds of GB of Arrow
 * payload. Enrichers publish files by rename; inode/mtime/size therefore
 * changes for every normal rewrite. A start/end mismatch makes a validation
 * run fail closed instead of combining two prepared-data states.
 */
async function fingerprintPreparedTree(root: string): Promise<Fingerprint> {
  const rootInfo = await stat(root)
  if (!rootInfo.isDirectory()) throw new Error(`${root} is not a directory`)

  const finder = spawn('find', [
    '-L', root, '-type', 'f',
    '-printf', '%P\t%s\t%T@\t%C@\t%D\t%i\n',
  ], { stdio: ['ignore', 'pipe', 'pipe'] })
  const sorter = spawn('sort', [], {
    env: { ...process.env, LC_ALL: 'C' },
    stdio: ['pipe', 'pipe', 'pipe'],
  })
  let pending = ''
  let metadataError = ''
  let newestChangeMs = 0
  const parseMetadata = (line: string) => {
    if (!line || metadataError) return
    const fields = line.split('\t')
    const mtimeMs = Number(fields.at(-4)) * 1000
    const ctimeMs = Number(fields.at(-3)) * 1000
    if (fields.length < 6 || !Number.isFinite(mtimeMs) || !Number.isFinite(ctimeMs)) {
      metadataError = `invalid prepared-data metadata from find: ${JSON.stringify(line.slice(0, 160))}`
      return
    }
    newestChangeMs = Math.max(newestChangeMs, mtimeMs, ctimeMs)
  }
  finder.stdout.setEncoding('utf8')
  finder.stdout.on('data', (chunk: string) => {
    pending += chunk
    let newline: number
    while ((newline = pending.indexOf('\n')) !== -1) {
      parseMetadata(pending.slice(0, newline))
      pending = pending.slice(newline + 1)
    }
  })
  const streamFailure = new Promise<never>((_resolve, reject) => {
    for (const [label, stream] of [
      ['find stdout', finder.stdout],
      ['sort stdin', sorter.stdin],
      ['sort stdout', sorter.stdout],
    ] as const) {
      stream.once('error', error => reject(new Error(`${label} failed during prepared-data fingerprint`, { cause: error })))
    }
  })
  finder.stdout.pipe(sorter.stdin)

  const hash = createHash('sha256')
  sorter.stdout.on('data', (chunk: Buffer) => hash.update(chunk))
  let finderError = ''
  let sorterError = ''
  finder.stderr.setEncoding('utf8').on('data', chunk => { finderError += chunk })
  sorter.stderr.setEncoding('utf8').on('data', chunk => { sorterError += chunk })

  const exit = (child: ChildProcess): Promise<number | null> => new Promise((resolveExit, reject) => {
    child.once('error', reject)
    child.once('close', resolveExit)
  })
  const completion = Promise.all([exit(finder), exit(sorter)])
  let finderCode: number | null
  let sorterCode: number | null
  try {
    [finderCode, sorterCode] = await Promise.race([completion, streamFailure])
  } catch (error) {
    finder.kill()
    sorter.kill()
    await completion.catch(() => {})
    throw error
  }
  parseMetadata(pending)
  if (finderCode !== 0) throw new Error(`prepared-data fingerprint failed: ${finderError.trim() || `find exit ${finderCode}`}`)
  if (sorterCode !== 0) throw new Error(`prepared-data fingerprint sort failed: ${sorterError.trim() || `sort exit ${sorterCode}`}`)
  if (metadataError) throw new Error(metadataError)
  return { sha256: hash.digest('hex'), newestChangeMs }
}

async function fingerprintPreparedInput(path: string): Promise<Fingerprint> {
  let info: Awaited<ReturnType<typeof stat>>
  try {
    info = await stat(path)
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') {
      return { sha256: createHash('sha256').update('<missing>').digest('hex'), newestChangeMs: 0 }
    }
    throw error
  }
  if (info.isDirectory()) return fingerprintPreparedTree(path)
  if (!info.isFile()) throw new Error(`${path} is not a regular file or directory`)
  const bytes = await readFile(path)
  const after = await stat(path)
  if (!sameFileIdentity(info, after)) throw new Error(`${path} changed while its prepared-input fingerprint was read`)
  return {
    sha256: createHash('sha256').update(bytes).digest('hex'),
    newestChangeMs: Math.max(after.mtimeMs, after.ctimeMs),
  }
}

async function fingerprintPreparedInputs(
  h3r4Dir: string,
  auxiliary: Array<{ label: string; path: string }>,
): Promise<Fingerprint> {
  const inputs = [{ label: 'h3r4', path: h3r4Dir }, ...auxiliary]
  const fingerprints = await Promise.all(inputs.map(async input => ({
    label: input.label,
    fingerprint: await fingerprintPreparedInput(input.path),
  })))
  const hash = createHash('sha256')
  let newestChangeMs = 0
  for (const { label, fingerprint } of fingerprints) {
    hash.update(label).update('\0').update(fingerprint.sha256).update('\0')
    newestChangeMs = Math.max(newestChangeMs, fingerprint.newestChangeMs)
  }
  return { sha256: hash.digest('hex'), newestChangeMs }
}

export function createValidationCohortProvider(options: ValidationCohortOptions = {}): ValidationCohortProvider {
  const cacheMs = options.cacheMs ?? 30_000
  if (!Number.isInteger(cacheMs) || cacheMs < 0 || cacheMs > 60_000) {
    throw new Error(`validation cohort cacheMs must be in 0..60000 (got ${cacheMs})`)
  }
  const dataYear = options.dataYear ?? DATA_YEAR
  const h3r4Dir = options.h3r4Dir ?? H3R4_DIR
  // Date.now() is integer milliseconds while stat carries sub-ms precision.
  // One millisecond is only a representation allowance, not a cache window.
  const modelProcessStartedAtMs = options.modelProcessStartedAtMs ?? Date.now() + 1
  const runtimeRoot = options.runtimeRoot ?? import.meta.dirname
  const sourceReaderPath = options.sourceReaderPath ?? SOURCE_READER_PATH
  const runtimeIdentityInputs = options.runtimeIdentityInputs ?? [
    { label: 'runtime-package', path: resolve(runtimeRoot, 'package.json') },
    { label: 'source-package-lock', path: resolve(runtimeRoot, '..', 'package-lock.json') },
    { label: 'runtime-dependency-lock', path: resolve(runtimeRoot, 'node_modules', '.package-lock.json') },
    { label: 'source-dependency-lock', path: resolve(runtimeRoot, '..', 'node_modules', '.package-lock.json') },
  ]
  const preparedRoot = resolve(h3r4Dir, '..', '..')
  const yearRoot = resolve(h3r4Dir, '..')
  // Admin needs no entry of its own: each cell's admin.bin lives inside the
  // h3r4 tree, which the `h3r4` input already fingerprints file by file.
  const preparedAuxiliaryInputs = options.preparedAuxiliaryInputs ?? [
    { label: 'dem', path: resolve(preparedRoot, 'dem') },
    { label: 'rasters', path: resolve(preparedRoot, 'rasters') },
    { label: 'airport-summary', path: resolve(yearRoot, 'aircraft', 'airport_summary.arrow') },
  ]
  let establishedCohortId: string | null = null
  let cached: CachedCohort | null = null
  let inFlight: Promise<ValidationCohort> | null = null

  return async () => {
    if (cached && Date.now() < cached.expiresAt) {
      if ('error' in cached) throw cached.error
      return cached.cohort
    }
    if (inFlight) return inFlight
    const computation = (async (): Promise<ValidationCohort> => {
      const [runtime, prepared] = await Promise.all([
        fingerprintRuntime(runtimeRoot, sourceReaderPath, runtimeIdentityInputs),
        fingerprintPreparedInputs(h3r4Dir, preparedAuxiliaryInputs),
      ])
      // source-reader keeps decoded H3 cells, rasters and admin data in
      // process-wide caches. A disk change cannot become a new valid cohort
      // in that same process: some queried cells may still be old. Fail closed
      // until restart instead of labelling cached results with the new hash.
      if (Math.max(runtime.newestChangeMs, prepared.newestChangeMs) > modelProcessStartedAtMs) {
        throw new Error('model code or prepared data changed after this server process loaded; restart required')
      }
      const runtimeSha256 = runtime.sha256
      const preparedSha256 = prepared.sha256
      const identity = JSON.stringify({ data_year: dataYear, runtime_sha256: runtimeSha256, prepared_sha256: preparedSha256 })
      const cohort: ValidationCohort = {
        schema_version: 1 as const,
        cohort_id: createHash('sha256').update(identity).digest('hex'),
        cache_ttl_ms: cacheMs,
        data_year: dataYear,
        runtime_sha256: runtimeSha256,
        prepared_sha256: preparedSha256,
      }
      if (establishedCohortId && cohort.cohort_id !== establishedCohortId) {
        throw new Error('model code or prepared data changed after this server established its cohort; restart required')
      }
      establishedCohortId = cohort.cohort_id
      return cohort
    })()
    inFlight = computation
    try {
      const cohort = await computation
      cached = { expiresAt: Date.now() + cacheMs, cohort }
      return cohort
    } catch (error) {
      // A stale process must not become a public stat-walk amplification path:
      // cache the fail-closed answer for the same short interval as success.
      cached = { expiresAt: Date.now() + cacheMs, error }
      throw error
    } finally {
      if (inFlight === computation) inFlight = null
    }
  }
}

export const currentValidationCohort = createValidationCohortProvider()
