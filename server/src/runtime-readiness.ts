// Readiness contract for the public server. Filesystem checks remain cheap
// and refresh periodically. Successful native probes have a short cache: this
// avoids making every monitor request contend with point queries while still
// detecting a worker that exits after startup.
import { createHash } from 'node:crypto'
import { join, relative, resolve, sep } from 'node:path'
import { readFile, stat } from 'node:fs/promises'
import { FRONTEND_DIST, PREPARED_YEAR_DIR, SOURCE_READER_PATH } from './runtime-paths.js'

export const READINESS_COMPONENTS = ['engine', 'frontend', 'prepared-data'] as const
export type ReadinessComponent = (typeof READINESS_COMPONENTS)[number]

export type ReadinessResult = {
  ready: boolean
  failed: ReadinessComponent[]
  /** Server logs only. Never include these strings in an HTTP response. */
  errors: Partial<Record<ReadinessComponent, string>>
}

export type ReadinessCheck = () => Promise<ReadinessResult>

type ReadinessOptions = {
  engineProbe: () => Promise<void>
  sourceReaderPath?: string
  frontendDist?: string
  preparedYearDir?: string
  now?: () => number
  filesystemCacheMs?: number
  engineRetryMs?: number
  engineSuccessCacheMs?: number
}

type FilesystemResult = Omit<ReadinessResult, 'ready'>

// Committed project reference square (Prague — grid::square_of(50.0, 14.25)).
// A fixed reference is deterministic and O(1); listing the prepared z9 tree
// allocates tens of thousands of entries on every readiness refresh.
export const REFERENCE_SQUARE = 'z9/276/173'

async function requireNonEmptyFile(filePath: string): Promise<void> {
  const info = await stat(filePath)
  if (!info.isFile() || info.size <= 0) {
    throw new Error(`${filePath} is not a non-empty regular file`)
  }
}

const FRONTEND_ASSET_MANIFEST = 'asset-manifest.json'
const SHA256 = /^[a-f0-9]{64}$/

// The map IS the product. The build writes one immutable inventory after Vite,
// precompression, and sitemap generation. Unlike index.html, that inventory
// includes lazy routes, web workers, fonts, and every other release artifact.
async function checkFrontend(frontendDist: string): Promise<void> {
  const manifestPath = join(frontendDist, FRONTEND_ASSET_MANIFEST)
  const manifest = JSON.parse(await readFile(manifestPath, 'utf8')) as {
    version?: unknown
    files?: Array<{ file?: unknown; bytes?: unknown; sha256?: unknown }>
  }
  if (manifest.version !== 1 || !Array.isArray(manifest.files) || manifest.files.length === 0) {
    throw new Error(`${manifestPath} has an invalid asset inventory`)
  }
  const root = resolve(frontendDist)
  const seen = new Set<string>()
  await Promise.all(manifest.files.map(async (entry) => {
    if (typeof entry.file !== 'string' || !entry.file || entry.file.includes('\\')
      || entry.file === FRONTEND_ASSET_MANIFEST || seen.has(entry.file)) {
      throw new Error(`${manifestPath} has an invalid or duplicate file name`)
    }
    seen.add(entry.file)
    if (!Number.isSafeInteger(entry.bytes) || (entry.bytes as number) <= 0
      || typeof entry.sha256 !== 'string' || !SHA256.test(entry.sha256)) {
      throw new Error(`${manifestPath} has invalid metadata for ${entry.file}`)
    }
    const assetPath = resolve(root, entry.file)
    const fromRoot = relative(root, assetPath)
    if (fromRoot === '' || fromRoot === '..' || fromRoot.startsWith(`..${sep}`)) {
      throw new Error(`${manifestPath} has an unsafe file name ${entry.file}`)
    }
    const contents = await readFile(assetPath)
    if (contents.length !== entry.bytes
      || createHash('sha256').update(contents).digest('hex') !== entry.sha256) {
      throw new Error(`${assetPath} does not match the frontend asset manifest`)
    }
  }))
  if (!seen.has('index.html')) throw new Error(`${manifestPath} does not contain index.html`)
  if (![...seen].some((file) => /^assets\/.*\.js$/.test(file))) {
    throw new Error(`${manifestPath} does not contain a JavaScript bundle`)
  }
}

async function checkPreparedData(preparedYearDir: string): Promise<void> {
  const root = await stat(preparedYearDir)
  if (!root.isDirectory()) throw new Error(`${preparedYearDir} is not a directory`)
  await requireNonEmptyFile(join(preparedYearDir, REFERENCE_SQUARE, 'roads.arrow'))
}

export function createReadinessCheck(options: ReadinessOptions): ReadinessCheck {
  const sourceReaderPath = options.sourceReaderPath ?? SOURCE_READER_PATH
  const frontendDist = options.frontendDist ?? FRONTEND_DIST
  const preparedYearDir = options.preparedYearDir ?? PREPARED_YEAR_DIR
  const now = options.now ?? Date.now
  const filesystemCacheMs = options.filesystemCacheMs ?? 10_000
  const engineRetryMs = options.engineRetryMs ?? 5_000
  const engineSuccessCacheMs = options.engineSuccessCacheMs ?? 10_000

  let filesystemCache: { expiresAt: number; result: FilesystemResult } | null = null
  let engineReadyUntil = 0
  let engineRetryAt = 0
  let engineError = 'native engine has not completed its readiness probe'
  let inFlight: Promise<ReadinessResult> | null = null

  async function checkFilesystem(): Promise<FilesystemResult> {
    const timestamp = now()
    if (filesystemCache && timestamp < filesystemCache.expiresAt) return filesystemCache.result

    const failed: ReadinessComponent[] = []
    const errors: ReadinessResult['errors'] = {}
    const checks: Array<[ReadinessComponent, () => Promise<void>]> = [
      ['engine', () => requireNonEmptyFile(sourceReaderPath)],
      ['frontend', () => checkFrontend(frontendDist)],
      ['prepared-data', () => checkPreparedData(preparedYearDir)],
    ]
    await Promise.all(checks.map(async ([component, check]) => {
      try {
        await check()
      } catch (error) {
        failed.push(component)
        errors[component] = error instanceof Error ? error.message : String(error)
      }
    }))
    failed.sort()
    const result = { failed, errors }
    // A transient startup failure must be observable again on the next probe;
    // start.sh retries readiness once a second. Successful stats are the only
    // results worth caching for the normal steady-state path.
    filesystemCache = failed.length === 0
      ? { expiresAt: timestamp + filesystemCacheMs, result }
      : null
    return result
  }

  return async () => {
    if (inFlight) return inFlight
    inFlight = (async () => {
      const filesystem = await checkFilesystem()
      const failed = new Set(filesystem.failed)
      const errors = { ...filesystem.errors }

      // Do not spawn/dlopen a worker against paths already known to be bad.
      if (!failed.has('engine') && !failed.has('prepared-data')) {
        let engineReady = now() < engineReadyUntil
        if (!engineReady && now() >= engineRetryAt) {
          try {
            await options.engineProbe()
            engineReady = true
            engineReadyUntil = now() + engineSuccessCacheMs
            engineError = ''
          } catch (error) {
            engineReadyUntil = 0
            engineError = error instanceof Error ? error.message : String(error)
            engineRetryAt = now() + engineRetryMs
          }
        }
        if (!engineReady) {
          failed.add('engine')
          errors.engine = engineError
        }
      }

      const ordered = [...failed].sort() as ReadinessComponent[]
      return { ready: ordered.length === 0, failed: ordered, errors }
    })().finally(() => {
      inFlight = null
    })
    return inFlight
  }
}
