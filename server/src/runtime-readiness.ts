// Readiness contract for the public server. Filesystem checks remain cheap
// and refresh periodically. Successful native probes have a short cache: this
// avoids making every monitor request contend with point queries while still
// detecting a worker that exits after startup.
import { createHash } from 'node:crypto'
import { constants } from 'node:fs'
import { lstat, open, readFile, stat, type FileHandle } from 'node:fs/promises'
import { basename, join, relative, resolve, sep } from 'node:path'
import {
  lineModelRoleSha256ForGeneration,
  validateGenerationContract,
  validatePublishedGenerationContract,
  validateQualificationClosureReference,
  validateTierGenerationAnchor,
} from './generation-contract.mjs'
import { ALLOWED_LAYERS, parseTierToken, PMTILES_BASE } from './routes/heatmap-shared.js'
import { FRONTEND_DIST, H3R4_DIR, SOURCE_READER_PATH } from './runtime-paths.js'
import { resolveManifestPath, resolveTileEnv, type TileEnv } from './tile-manifest-reader.js'

export const READINESS_COMPONENTS = ['engine', 'frontend', 'prepared-data', 'pmtiles'] as const
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
  h3r4Dir?: string
  pmtilesDir?: string
  /** Overrides process.env.TILE_ENV — tests only; see resolveTileEnv's own doc. */
  tileEnv?: string
  now?: () => number
  filesystemCacheMs?: number
  engineRetryMs?: number
  engineSuccessCacheMs?: number
}

type FilesystemResult = Omit<ReadinessResult, 'ready'>

const REFERENCE_HEX = '841e309ffffffff' // Dobříš — committed project reference cell.

async function requireNonEmptyFile(filePath: string): Promise<void> {
  const info = await stat(filePath)
  if (!info.isFile() || info.size <= 0) {
    throw new Error(`${filePath} is not a non-empty regular file`)
  }
}

const FRONTEND_ASSET_MANIFEST = 'asset-manifest.json'
const SHA256 = /^[a-f0-9]{64}$/
const MAX_QUALIFICATION_CLOSURE_BYTES = 32 * 1024 * 1024

type OpenedRegularFile = {
  descriptor: FileHandle
  info: {
    dev: bigint
    ino: bigint
    size: bigint
    mtimeNs: bigint
    ctimeNs: bigint
    isFile: () => boolean
  }
}

/** Open one regular leaf without following its final path component. */
async function openRegularFileNoFollow(path: string, label = path): Promise<OpenedRegularFile> {
  let descriptor: FileHandle | undefined
  let keepOpen = false
  try {
    descriptor = await open(
      path,
      constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK,
    )
    const info = await descriptor.stat({ bigint: true })
    if (!info.isFile()) throw new Error(`${label} is not a regular file`)
    keepOpen = true
    return { descriptor, info }
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ELOOP') {
      throw new Error(`${label} is not a regular file`)
    }
    throw error
  } finally {
    if (!keepOpen) await descriptor?.close().catch(() => {})
  }
}

function fileIdentity(info: OpenedRegularFile['info']): string {
  return `${info.dev}:${info.ino}:${info.size}:${info.mtimeNs}:${info.ctimeNs}`
}

/** Stat an archive leaf without following a symlink; readiness remains stat-only. */
async function statRegularFileNoFollow(path: string, label = path) {
  const info = await lstat(path, { bigint: true })
  if (!info.isFile()) throw new Error(`${label} is not a regular file`)
  return info
}

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

async function checkPreparedData(h3r4Dir: string): Promise<void> {
  const root = await stat(h3r4Dir)
  if (!root.isDirectory()) throw new Error(`${h3r4Dir} is not a directory`)
  // A fixed reference is deterministic and O(1); listing the global H3 tree
  // allocates tens of thousands of entries on every readiness refresh.
  await requireNonEmptyFile(join(h3r4Dir, REFERENCE_HEX, 'roads.arrow'))
}

// publisher_proof still EXISTS in manifests (tile-store-pack writes it; fsck and
// --rebind-verified consume it) — this reader just no longer judges it, so it has
// no type here. See the decision comment inside checkPmtiles().
export type ManifestLayer = {
  file?: unknown
  bytes?: unknown
  build?: unknown
  sha256?: unknown
}

const BUILD_ID = /^b[0-9]+$/
export type PmtilesManifest = {
    build?: unknown
    generation?: unknown
    layers?: Record<string, ManifestLayer>
    qualification_closure?: unknown
    [key: string]: unknown
}

/** A generation is never valid without its matching top-level line identity. */
function generationFencedManifest(manifest: PmtilesManifest, manifestPath: string): boolean {
  const hasGeneration = manifest.generation !== undefined
  const hasLineIdentity = manifest.line_model_role_sha256 !== undefined
  if (hasGeneration && !hasLineIdentity) {
    throw new Error(`${manifestPath} mixes legacy and generation-fenced manifest fields`)
  }
  return hasGeneration
}

/** Validate one already-parsed environment pin with the exact boot-readiness contract. */
export async function validatePmtilesManifest(
  manifest: PmtilesManifest,
  pmtilesDir: string,
  manifestPath: string,
  tileEnv: TileEnv = 'prod',
): Promise<void> {
  if (typeof manifest.build !== 'string' || !BUILD_ID.test(manifest.build)) {
    throw new Error(`${manifestPath} has an invalid build id`)
  }
  if (!manifest.layers || typeof manifest.layers !== 'object') {
    throw new Error(`${manifestPath} has no layers object`)
  }
  if (generationFencedManifest(manifest, manifestPath)) {
    try {
      const generation = tileEnv === 'prod'
        ? validatePublishedGenerationContract(manifest.generation)
        : validateGenerationContract(manifest.generation)
      if (generation.tier !== '') {
        throw new Error('top-level generation must be a base contract')
      }
      if (manifest.line_model_role_sha256
          !== lineModelRoleSha256ForGeneration(generation)) {
        throw new Error('line_model_role_sha256 differs from the base generation')
      }
    } catch (error) {
      throw new Error(`${manifestPath} has an invalid generation contract: ${(error as Error).message}`)
    }
  }

  for (const layer of ALLOWED_LAYERS) {
    if (!(layer in manifest.layers)) {
      throw new Error(`${manifestPath} is missing layer ${layer}`)
    }
  }
  for (const [layer, entry] of Object.entries(manifest.layers)) {
    if (!ALLOWED_LAYERS.has(layer) && parseTierToken(layer) === null) {
      throw new Error(`${manifestPath} has unexpected layer ${layer}`)
    }
    if (!entry || typeof entry.file !== 'string' || basename(entry.file) !== entry.file) {
      throw new Error(`${manifestPath} layer ${layer} has an unsafe file name`)
    }
    const expectedPrefix = `${layer}.`
    const filenameBuild = entry.file.startsWith(expectedPrefix) && entry.file.endsWith('.pmtiles')
      ? entry.file.slice(expectedPrefix.length, -'.pmtiles'.length)
      : ''
    if (!BUILD_ID.test(filenameBuild)) {
      throw new Error(`${manifestPath} layer ${layer} file does not match the served archive name`)
    }
    if (entry.build !== undefined) {
      if (typeof entry.build !== 'string' || !BUILD_ID.test(entry.build)) {
        throw new Error(`${manifestPath} layer ${layer} has an invalid build id`)
      }
      if (entry.build !== filenameBuild) {
        throw new Error(`${manifestPath} layer ${layer} build does not match its archive file`)
      }
    }
    if (!Number.isSafeInteger(entry.bytes) || (entry.bytes as number) <= 0) {
      throw new Error(`${manifestPath} layer ${layer} has invalid bytes`)
    }
    if (typeof entry.sha256 !== 'string' || !SHA256.test(entry.sha256)) {
      throw new Error(`${manifestPath} layer ${layer} has invalid sha256`)
    }
    // DELIBERATELY NO stat-identity (dev/ino/ctime) verification here any more (owner call,
    // 2026-07-15). The strict identity gate ("sha256-posix-stat-v1" proof ↔ live stat) shipped
    // 2026-07-14 and its production record was: real faults caught 0, deploys broken 1 — the
    // planned Track D relocation of pmtiles/ to /data1 legitimately changed dev+ino+ctime of
    // every archive, readiness went permanently red, and start.sh could no longer activate any
    // release until a 330 GB content re-hash re-bound the proofs. This project MOVES its data
    // on purpose (disk rebalances, the planned second server), so inode-identity fires exactly
    // during planned maintenance — while the incident class that motivated it (the packer
    // writing corrupt bytes, 2026-07-13) is structurally uncatchable by ANY post-publish check
    // that trusts the packer's own attestation. Content integrity lives where reading the
    // bytes is worth it: pack-time validation (tile-store-pack's decode checks), fsck, and the
    // on-demand --rebind-verified full re-hash. Boot-time readiness keeps the cheap invariants
    // that catch real operational mistakes: manifest coherence, per-layer completeness, safe
    // archive names, and exact size match (truncation / wrong-file swap).
    const archivePath = join(pmtilesDir, entry.file)
    const archive = await statRegularFileNoFollow(archivePath)
    if (archive.size !== BigInt(entry.bytes as number)) {
      throw new Error(`${archivePath} size ${archive.size} does not match manifest ${entry.bytes}`)
    }
  }
  validateTiersIndex(manifest, manifestPath)
  validateTierGenerationProfiles(manifest, manifestPath, tileEnv)
  await validateManifestQualificationClosure(manifest, pmtilesDir, manifestPath)
}

async function validateManifestQualificationClosure(
  manifest: PmtilesManifest,
  pmtilesDir: string,
  manifestPath: string,
): Promise<void> {
  const generationFenced = manifest.generation !== undefined
  const tiered = manifest.tiers !== undefined
  if (!generationFenced || !tiered) {
    if (manifest.qualification_closure !== undefined) {
      throw new Error(`${manifestPath} must not carry a qualification closure without fenced tiers`)
    }
    return
  }
  // Qualification closures are legacy campaign evidence (removed from the packer
  // 2026-08-28): new tier packs carry none, old archives still do. Validate one only
  // when it is present; its absence on a tiered manifest is not a readiness error.
  if (manifest.qualification_closure === undefined) {
    return
  }
  let reference: { file: string; sha256: string }
  try {
    reference = validateQualificationClosureReference(manifest.qualification_closure)
  } catch (error) {
    throw new Error(`${manifestPath} has an invalid qualification closure: ${(error as Error).message}`)
  }
  const closurePath = join(pmtilesDir, reference.file)
  let descriptor
  try {
    descriptor = await open(
      closurePath,
      constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK,
    )
    const info = await descriptor.stat({ bigint: true })
    if (!info.isFile()) {
      throw new Error(`${closurePath} is not a regular file`)
    }
    if (info.size <= 0n || info.size > BigInt(MAX_QUALIFICATION_CLOSURE_BYTES)) {
      throw new Error(`${closurePath} has an invalid byte count`)
    }
    if ((info.mode & 0o222n) !== 0n) {
      throw new Error(`${closurePath} is writable`)
    }
    const bytes = await descriptor.readFile()
    if (createHash('sha256').update(bytes).digest('hex') !== reference.sha256) {
      throw new Error(`${closurePath} sha256 differs from the manifest`)
    }
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ELOOP') {
      throw new Error(`${closurePath} is not a regular file`)
    }
    throw error
  } finally {
    await descriptor?.close()
  }
}

/** Referential integrity of the zoom-tier index (city-z13 plan §D): a torn
 *  `tiers` object must fail readiness, not silently mis-serve — the serving
 *  resolver decides authoritative silence from it. Keep in lockstep with the
 *  private ops copy (validate-manifest.mjs). */
export function validateTiersIndex(
  manifest: PmtilesManifest,
  manifestPath: string,
): void {
  const tiers = manifest.tiers
  if (tiers === undefined) {
    for (const layer of Object.keys(manifest.layers || {})) {
      if (parseTierToken(layer) !== null) {
        throw new Error(`${manifestPath} tier token ${layer} is absent from the tiers index`)
      }
    }
    return
  }
  if (!tiers || typeof tiers !== 'object' || Array.isArray(tiers)) {
    throw new Error(`${manifestPath} tiers is not an object`)
  }
  const generationFenced = generationFencedManifest(manifest, manifestPath)
  let baseGeneration: ReturnType<typeof validateGenerationContract> | null = null
  if (generationFenced) {
    try {
      baseGeneration = validateGenerationContract(manifest.generation)
    } catch (error) {
      throw new Error(`${manifestPath} tiers has an invalid base generation: ${(error as Error).message}`)
    }
  }
  const indexedTierTokens = new Set<string>()
  for (const [zoom, entry] of Object.entries(tiers as Record<string, { packs?: unknown }>)) {
    const zoomNum = /^z(1[3-8])$/.exec(zoom)?.[1]
    if (!zoomNum) throw new Error(`${manifestPath} tiers has invalid zoom key ${zoom}`)
    if (!Array.isArray(entry?.packs)) throw new Error(`${manifestPath} tiers.${zoom} has no packs array`)
    const seen = new Set<string>()
    for (const p of entry.packs as Array<Record<string, unknown>>) {
      if (typeof p?.pack !== 'string' || !/^p[0-9]+$/.test(p.pack)) {
        throw new Error(`${manifestPath} tiers.${zoom} has a non-canonical pack id`)
      }
      if (seen.has(p.pack)) throw new Error(`${manifestPath} tiers.${zoom} pack ${p.pack} is duplicated`)
      seen.add(p.pack)
      if (generationFenced) {
        try {
          validateTierGenerationAnchor(baseGeneration, p.generation, zoom)
        } catch (error) {
          throw new Error(
            `${manifestPath} tiers.${zoom} pack ${p.pack} has an invalid generation: ${(error as Error).message}`,
          )
        }
      } else if (p.generation !== undefined) {
        throw new Error(`${manifestPath} legacy tier pack ${p.pack} carries a generation identity`)
      }
      if (!Array.isArray(p.coverage_r4) || p.coverage_r4.length === 0
        || !p.coverage_r4.every((c) => typeof c === 'string' && /^84[0-9a-f]{5}ffffffff$/.test(c))) {
        throw new Error(`${manifestPath} tiers.${zoom} pack ${p.pack} has invalid coverage_r4`)
      }
      if (!Array.isArray(p.layers) || p.layers.length === 0) {
        throw new Error(`${manifestPath} tiers.${zoom} pack ${p.pack} has no layers list`)
      }
      const expectedTokens = [...ALLOWED_LAYERS]
        .map(layer => `${layer}-${zoom}-${p.pack}`)
        .sort()
      const observedTokens = p.layers.every(token => typeof token === 'string')
        ? [...p.layers as string[]].sort()
        : []
      for (const token of p.layers) {
        const parsed = typeof token === 'string' ? parseTierToken(token) : null
        if (!parsed || String(parsed.tier) !== zoomNum || parsed.pack !== p.pack) {
          throw new Error(`${manifestPath} tiers.${zoom} pack ${p.pack} lists foreign token ${String(token)}`)
        }
      }
      if (observedTokens.length !== expectedTokens.length
        || observedTokens.some((token, index) => token !== expectedTokens[index])) {
        throw new Error(
          `${manifestPath} tiers.${zoom} pack ${p.pack} does not contain the exact 8-layer bundle`,
        )
      }
      for (const token of p.layers) {
        if (!manifest.layers || typeof manifest.layers[token as string] !== 'object') {
          throw new Error(`${manifestPath} tiers.${zoom} pack ${p.pack} token ${String(token)} has no layers entry`)
        }
        if (indexedTierTokens.has(token as string)) {
          throw new Error(`${manifestPath} tier token ${String(token)} is indexed more than once`)
        }
        indexedTierTokens.add(token as string)
      }
    }
  }
  for (const layer of Object.keys(manifest.layers || {})) {
    if (parseTierToken(layer) !== null && !indexedTierTokens.has(layer)) {
      throw new Error(`${manifestPath} tier token ${layer} is absent from the tiers index`)
    }
  }
}

/** Development serves structurally valid experiments; production accepts only selected profiles. */
function validateTierGenerationProfiles(
  manifest: PmtilesManifest,
  manifestPath: string,
  tileEnv: TileEnv,
): void {
  if (manifest.generation === undefined || manifest.tiers === undefined) return
  for (const [zoom, entry] of Object.entries(
    manifest.tiers as Record<string, { packs: Array<Record<string, unknown>> }>,
  )) {
    for (const pack of entry.packs) {
      try {
        if (tileEnv === 'prod') validatePublishedGenerationContract(pack.generation)
        else validateGenerationContract(pack.generation)
      } catch (error) {
        throw new Error(
          `${manifestPath} tiers.${zoom} pack ${String(pack.pack)} has an unsupported published generation: ${(error as Error).message}`,
        )
      }
    }
  }
}

/** Read and validate one per-environment pin; shared by readiness and the public manifest route. */
export class PmtilesManifestPinMissingError extends Error {
  readonly code = 'QM_PMTILES_MANIFEST_PIN_MISSING'
}

async function openManifestPinNoFollow(path: string): Promise<OpenedRegularFile> {
  try {
    return await openRegularFileNoFollow(path)
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') {
      throw new PmtilesManifestPinMissingError(`${path} does not exist`)
    }
    throw error
  }
}

const PMTILES_MANIFEST_CACHE_MS = 10_000
type CachedPmtilesManifest = {
  identity: string
  validatedAt: number
  manifest: PmtilesManifest
}
const pmtilesManifestCache = new Map<string, CachedPmtilesManifest>()
const pmtilesManifestLoads = new Map<string, Promise<PmtilesManifest>>()

async function readValidatedPmtilesManifestFile(
  pmtilesDir: string,
  manifestPath: string,
  opened: OpenedRegularFile,
  tileEnv: TileEnv,
): Promise<{ manifest: PmtilesManifest; identity: string }> {
  const raw = (await opened.descriptor.readFile()).toString('utf8')
  const manifest = JSON.parse(raw) as PmtilesManifest
  await validatePmtilesManifest(manifest, pmtilesDir, manifestPath, tileEnv)
  const identity = fileIdentity(await opened.descriptor.stat({ bigint: true }))
  return { manifest, identity }
}

export async function readValidatedPmtilesManifest(
  pmtilesDir: string,
  tileEnv?: string,
): Promise<PmtilesManifest> {
  // Per-environment pin (docs/dev/checkout-restructure-plan.md Track 2): boot readiness and the
  // route both gate on THIS deployment's pin, never the packer's shared merge head.
  const resolvedTileEnv = resolveTileEnv(tileEnv)
  const manifestPath = resolveManifestPath(pmtilesDir, resolvedTileEnv)
  const opened = await openManifestPinNoFollow(manifestPath)
  try {
    return (await readValidatedPmtilesManifestFile(
      pmtilesDir, manifestPath, opened, resolvedTileEnv,
    )).manifest
  } finally {
    await opened.descriptor.close()
  }
}

/** Share one short-lived, stat-bound validation across readiness and public manifest routes. */
export async function readCachedValidatedPmtilesManifest(
  pmtilesDir: string,
  tileEnv?: string,
): Promise<PmtilesManifest> {
  const resolvedTileEnv = resolveTileEnv(tileEnv)
  const manifestPath = resolveManifestPath(pmtilesDir, resolvedTileEnv)
  for (let attempt = 0; attempt < 2; attempt++) {
    const opened = await openManifestPinNoFollow(manifestPath)
    const identity = fileIdentity(opened.info)
    const now = Date.now()
    const cached = pmtilesManifestCache.get(manifestPath)
    if (cached?.identity === identity
        && now - cached.validatedAt < PMTILES_MANIFEST_CACHE_MS) {
      await opened.descriptor.close()
      return cached.manifest
    }

    const loadKey = `${manifestPath}\0${identity}`
    let loading = pmtilesManifestLoads.get(loadKey)
    if (!loading) {
      loading = (async () => {
        try {
          const { manifest, identity: identityAfter } =
            await readValidatedPmtilesManifestFile(
              pmtilesDir, manifestPath, opened, resolvedTileEnv,
            )
          if (identityAfter !== identity) {
            throw new Error(`${manifestPath} changed while it was being validated`)
          }
          pmtilesManifestCache.set(manifestPath, {
            identity,
            validatedAt: Date.now(),
            manifest,
          })
          return manifest
        } finally {
          await opened.descriptor.close()
        }
      })().finally(() => pmtilesManifestLoads.delete(loadKey))
      pmtilesManifestLoads.set(loadKey, loading)
    } else {
      await opened.descriptor.close()
    }
    try {
      return await loading
    } catch (error) {
      if (attempt === 0
          && (error as Error).message === `${manifestPath} changed while it was being validated`) {
        continue
      }
      throw error
    }
  }
  throw new Error(`${manifestPath} changed repeatedly while it was being validated`)
}

async function checkPmtiles(pmtilesDir: string, tileEnv?: string): Promise<void> {
  await readCachedValidatedPmtilesManifest(pmtilesDir, tileEnv)
}

export function createReadinessCheck(options: ReadinessOptions): ReadinessCheck {
  const sourceReaderPath = options.sourceReaderPath ?? SOURCE_READER_PATH
  const frontendDist = options.frontendDist ?? FRONTEND_DIST
  const h3r4Dir = options.h3r4Dir ?? H3R4_DIR
  const pmtilesDir = options.pmtilesDir ?? PMTILES_BASE
  const tileEnv = options.tileEnv
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
      ['prepared-data', () => checkPreparedData(h3r4Dir)],
      ['pmtiles', () => checkPmtiles(pmtilesDir, tileEnv)],
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
