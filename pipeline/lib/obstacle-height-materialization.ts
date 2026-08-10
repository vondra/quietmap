/**
 * Freshness proof for each materialized obstacle-height cell.
 *
 * `obstacles.arrow` is rewritten outside the enrichment chain when Overture
 * staging is promoted. The adjacent proof binds the enriched output inode to
 * every input that can change its height ladder, so a later chain run can
 * distinguish a current cell from a silently reverted one without reading the
 * Arrow payload.
 */

import { createHash } from 'node:crypto'
import { basename, dirname, extname, isAbsolute, relative, resolve } from 'node:path'
import { readFileSync, statSync } from 'node:fs'

export const OBSTACLE_HEIGHT_PROOF_FILENAME = 'obstacles.height-materialization.json'
export const OBSTACLE_HEIGHT_PROOF_VERSION = 1

export interface StableFileIdentity {
  dev: string
  ino: string
  size: string
  mtimeNs: string
  ctimeNs: string
}

export interface ObstacleHeightProof {
  version: number
  cell: string
  inputsSha256: string
  output: StableFileIdentity
  rows: number
  beforeTiers: readonly number[]
  afterTiers: readonly number[]
}

export interface ObstacleHeightCandidate {
  cell: string
  outputPath: string
  proofPath: string
  inputsSha256: string
}

export interface ObstacleHeightSharedInputContract {
  version: number
  workerSha256: string
  ghslIdentity: StableFileIdentity
  regional: unknown
}

export interface ObstacleHeightProofState {
  current: boolean
  reason: 'current' | 'missing-proof' | 'invalid-proof' | 'input-changed' | 'output-replaced'
}

export function stableFileIdentity(path: string): StableFileIdentity {
  const s = statSync(path, { bigint: true })
  return {
    dev: s.dev.toString(),
    ino: s.ino.toString(),
    size: s.size.toString(),
    mtimeNs: s.mtimeNs.toString(),
    ctimeNs: s.ctimeNs.toString(),
  }
}

function identitiesMatch(a: StableFileIdentity, b: StableFileIdentity): boolean {
  return a.dev === b.dev && a.ino === b.ino && a.size === b.size && a.mtimeNs === b.mtimeNs && a.ctimeNs === b.ctimeNs
}

function sha256(text: string | Buffer): string {
  return createHash('sha256').update(text).digest('hex')
}

function decodeXmlPath(path: string): string {
  return path
    .replaceAll('&amp;', '&')
    .replaceAll('&lt;', '<')
    .replaceAll('&gt;', '>')
    .replaceAll('&quot;', '"')
    .replaceAll('&apos;', "'")
}

/** Bind a VRT to both its XML and every source raster it names. Replacing a
 * source tile without rewriting the VRT must still invalidate the proof. */
function regionalRasterContract(vrtPath: string): unknown {
  if (extname(vrtPath).toLowerCase() !== '.vrt') {
    return { rasterIdentity: stableFileIdentity(vrtPath) }
  }
  const xml = readFileSync(vrtPath, 'utf8')
  const sourcePaths = [...xml.matchAll(/<SourceFilename\b[^>]*>([^<]+)<\/SourceFilename>/g)]
    .map((m) => decodeXmlPath(m[1]))
    .map((p) => (isAbsolute(p) ? p : resolve(dirname(vrtPath), p)))
  const unique = [...new Set(sourcePaths)].sort()
  return {
    vrtSha256: sha256(xml),
    vrtIdentity: stableFileIdentity(vrtPath),
    sources: unique.map((path) => ({
      path: relative(dirname(vrtPath), path),
      identity: stableFileIdentity(path),
    })),
  }
}

/** Digest of every input whose change requires rematerializing one cell. Paths
 * themselves are reduced to stable role/name labels so proofs remain valid
 * across checkouts that share the same physical data node. */
export function obstacleHeightSharedInputContract(args: {
  workerPath: string
  ghslPath: string
  regionalVrtPath: string | null
}): ObstacleHeightSharedInputContract {
  return {
    version: OBSTACLE_HEIGHT_PROOF_VERSION,
    workerSha256: sha256(readFileSync(args.workerPath)),
    ghslIdentity: stableFileIdentity(args.ghslPath),
    regional: args.regionalVrtPath ? regionalRasterContract(args.regionalVrtPath) : null,
  }
}

export function obstacleHeightInputsSha256(args: {
  shared: ObstacleHeightSharedInputContract
  stagingShardPaths: readonly string[]
}): string {
  const staging = [...args.stagingShardPaths].sort().map((path) => ({
    name: basename(path),
    identity: stableFileIdentity(path),
  }))
  const contract = {
    shared: args.shared,
    staging,
  }
  return sha256(JSON.stringify(contract))
}

export function obstacleHeightProofState(candidate: ObstacleHeightCandidate): ObstacleHeightProofState {
  let proof: ObstacleHeightProof
  try {
    proof = JSON.parse(readFileSync(candidate.proofPath, 'utf8')) as ObstacleHeightProof
  } catch (err) {
    if (err && typeof err === 'object' && 'code' in err && (err as NodeJS.ErrnoException).code === 'ENOENT') {
      return { current: false, reason: 'missing-proof' }
    }
    return { current: false, reason: 'invalid-proof' }
  }
  if (
    proof.version !== OBSTACLE_HEIGHT_PROOF_VERSION ||
    proof.cell !== candidate.cell ||
    typeof proof.inputsSha256 !== 'string' ||
    !proof.output ||
    !Number.isInteger(proof.rows) ||
    proof.rows < 0 ||
    !Array.isArray(proof.beforeTiers) ||
    proof.beforeTiers.length !== 5 ||
    !Array.isArray(proof.afterTiers) ||
    proof.afterTiers.length !== 5
  ) {
    return { current: false, reason: 'invalid-proof' }
  }
  if (proof.inputsSha256 !== candidate.inputsSha256) return { current: false, reason: 'input-changed' }
  try {
    return identitiesMatch(proof.output, stableFileIdentity(candidate.outputPath))
      ? { current: true, reason: 'current' }
      : { current: false, reason: 'output-replaced' }
  } catch {
    return { current: false, reason: 'output-replaced' }
  }
}

export function selectStaleObstacleHeightCells(candidates: readonly ObstacleHeightCandidate[]): ObstacleHeightCandidate[] {
  return candidates.filter((candidate) => !obstacleHeightProofState(candidate).current)
}
