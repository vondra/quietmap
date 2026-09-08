//! Atomic gzip-NDJSON reference artifacts with manifest hash verification.

import { createHash } from 'node:crypto'
import { once } from 'node:events'
import { createReadStream, createWriteStream } from 'node:fs'
import { chmod, mkdir, mkdtemp, readFile, rename, rm, stat, writeFile } from 'node:fs/promises'
import { dirname, basename, join, resolve, sep } from 'node:path'
import { createInterface } from 'node:readline'
import { pipeline } from 'node:stream/promises'
import { Transform } from 'node:stream'
import { createGunzip, createGzip } from 'node:zlib'

export const ARTIFACT_FILE = 'responses.ndjson.gz'
export const MANIFEST_FILE = 'manifest.json'
const SUPPORTED_KINDS = new Set([
  'quietmap-popup-reference',
  'quietmap-popup-browser-reference',
  'quietmap-popup-browser-candidate',
])

async function exists(path) {
  try {
    await stat(path)
    return true
  } catch (error) {
    if (error.code === 'ENOENT') return false
    throw error
  }
}

async function sha256File(path) {
  const hash = createHash('sha256')
  for await (const chunk of createReadStream(path)) hash.update(chunk)
  return hash.digest('hex')
}

export async function createCaptureWriter(outputPath, expectedEntries) {
  const destination = resolve(outputPath)
  if (await exists(destination)) throw new Error(`output already exists: ${destination}`)
  const parent = dirname(destination)
  await mkdir(parent, { recursive: true })
  const temporary = await mkdtemp(join(parent, `.${basename(destination)}.partial-`))
  const artifactPath = join(temporary, ARTIFACT_FILE)
  const output = createWriteStream(artifactPath, { flags: 'wx' })
  const gzip = createGzip({ level: 9 })
  const completed = pipeline(gzip, output)
  const uncompressedHash = createHash('sha256')
  const pending = new Map()
  let writeChain = Promise.resolve()
  let nextIndex = 0
  let aborted = false
  const attachments = []
  const attachmentNames = new Set()

  async function flush() {
    while (pending.has(nextIndex)) {
      const entry = pending.get(nextIndex)
      pending.delete(nextIndex)
      const line = `${JSON.stringify(entry)}\n`
      uncompressedHash.update(line)
      if (!gzip.write(line)) await once(gzip, 'drain')
      nextIndex += 1
    }
  }

  return {
    destination,
    append(index, entry) {
      if (aborted) throw new Error('capture writer is aborted')
      if (!Number.isInteger(index) || index < nextIndex || pending.has(index)) {
        throw new Error(`duplicate or stale capture index: ${index}`)
      }
      pending.set(index, entry)
      writeChain = writeChain.then(flush)
      return writeChain
    },
    async writeAttachment(relativePath, body, contentType) {
      if (aborted) throw new Error('capture writer is aborted')
      if (typeof relativePath !== 'string' || relativePath.length === 0) {
        throw new Error('attachment path must be a non-empty string')
      }
      const path = resolve(temporary, relativePath)
      if (!path.startsWith(`${temporary}${sep}`)
          || [ARTIFACT_FILE, MANIFEST_FILE].includes(relativePath)
          || attachmentNames.has(relativePath)) {
        throw new Error(`unsafe or duplicate attachment path: ${relativePath}`)
      }
      const bytes = Buffer.isBuffer(body) ? body : Buffer.from(body)
      await mkdir(dirname(path), { recursive: true })
      await writeFile(path, bytes, { flag: 'wx' })
      await chmod(path, 0o444)
      const record = {
        file: relativePath,
        content_type: contentType,
        bytes: bytes.length,
        sha256: createHash('sha256').update(bytes).digest('hex'),
      }
      attachmentNames.add(relativePath)
      attachments.push(record)
      return record
    },
    async finish(manifestFields) {
      await writeChain
      if (pending.size !== 0 || nextIndex !== expectedEntries) {
        throw new Error(`capture entry count ${nextIndex} does not match expected ${expectedEntries}`)
      }
      gzip.end()
      await completed
      const artifactStat = await stat(artifactPath)
      const manifest = {
        schema_version: 1,
        kind: 'quietmap-popup-reference',
        ...manifestFields,
        ...(attachments.length ? { attachments: [...attachments].sort((a, b) => a.file.localeCompare(b.file)) } : {}),
        artifact: {
          file: ARTIFACT_FILE,
          encoding: 'gzip-ndjson',
          entry_count: nextIndex,
          compressed_bytes: artifactStat.size,
          compressed_sha256: await sha256File(artifactPath),
          uncompressed_sha256: uncompressedHash.digest('hex'),
        },
      }
      const manifestPath = join(temporary, MANIFEST_FILE)
      await writeFile(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`, { flag: 'wx' })
      await chmod(artifactPath, 0o444)
      await chmod(manifestPath, 0o444)
      await rename(temporary, destination)
      return { manifest, manifestPath: join(destination, MANIFEST_FILE) }
    },
    async abort() {
      if (aborted) return
      aborted = true
      gzip.destroy()
      await completed.catch(() => {})
      await rm(temporary, { recursive: true, force: true })
    },
  }
}

async function manifestPathFromInput(inputPath) {
  const resolved = resolve(inputPath)
  const info = await stat(resolved)
  return info.isDirectory() ? join(resolved, MANIFEST_FILE) : resolved
}

function validateManifest(value, path) {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error(`${path}: expected object`)
  }
  const required = [
    'schema_version', 'kind', 'created_at', 'source', 'points',
    'request_policy', 'coverage', 'artifact',
  ]
  const optional = ['attachments', 'browser', 'render_plan', 'comparison']
  const keys = Object.keys(value)
  if (required.some((key) => !keys.includes(key))
      || keys.some((key) => !required.includes(key) && !optional.includes(key))) {
    throw new Error(`${path}: manifest schema keys changed`)
  }
  if (value.schema_version !== 1 || !SUPPORTED_KINDS.has(value.kind)) {
    throw new Error(`${path}: unsupported reference artifact`)
  }
  if (value.artifact?.file !== ARTIFACT_FILE || value.artifact?.encoding !== 'gzip-ndjson') {
    throw new Error(`${path}: unsupported artifact encoding`)
  }
  for (const key of ['compressed_sha256', 'uncompressed_sha256']) {
    if (!/^[0-9a-f]{64}$/.test(value.artifact[key])) {
      throw new Error(`${path}: invalid artifact ${key}`)
    }
  }
  if (!Number.isInteger(value.artifact.entry_count) || value.artifact.entry_count < 1) {
    throw new Error(`${path}: invalid artifact entry_count`)
  }
  if (value.attachments !== undefined) {
    if (!Array.isArray(value.attachments)) throw new Error(`${path}: attachments must be an array`)
    const names = new Set()
    for (const attachment of value.attachments) {
      const attachmentKeys = ['file', 'content_type', 'bytes', 'sha256']
      if (attachment === null || typeof attachment !== 'object' || Array.isArray(attachment)
          || Object.keys(attachment).sort().join() !== attachmentKeys.sort().join()) {
        throw new Error(`${path}: attachment schema changed`)
      }
      if (typeof attachment.file !== 'string' || attachment.file.length === 0
          || attachment.file.startsWith('/') || attachment.file.split('/').includes('..')
          || names.has(attachment.file)) {
        throw new Error(`${path}: unsafe or duplicate attachment path`)
      }
      if (typeof attachment.content_type !== 'string' || !Number.isInteger(attachment.bytes)
          || attachment.bytes < 0 || !/^[0-9a-f]{64}$/.test(attachment.sha256)) {
        throw new Error(`${path}: invalid attachment metadata`)
      }
      names.add(attachment.file)
    }
  }
  return value
}

function validateEntry(value, expectedIndex, path) {
  const keys = [
    'schema_version', 'index', 'point', 'instance', 'http_status',
    'content_type', 'elapsed_ms', 'body_sha256', 'payload',
  ]
  if (value === null || typeof value !== 'object' || Array.isArray(value)
      || Object.keys(value).some((key) => !keys.includes(key) && key !== 'browser')
      || keys.some((key) => !Object.hasOwn(value, key))) {
    throw new Error(`${path}: capture entry schema changed`)
  }
  if (value.schema_version !== 1 || value.index !== expectedIndex) {
    throw new Error(`${path}: non-consecutive capture index`)
  }
  if (!/^[0-9a-f]{64}$/.test(value.body_sha256)) throw new Error(`${path}: invalid body_sha256`)
  return value
}

export async function openReferenceArtifact(inputPath, expectedPointHashes) {
  const manifestPath = await manifestPathFromInput(inputPath)
  const manifest = validateManifest(JSON.parse(await readFile(manifestPath, 'utf8')), manifestPath)
  for (const [key, expected] of Object.entries(expectedPointHashes)) {
    if (manifest.points.hashes?.[key] !== expected) {
      throw new Error(`${manifestPath}: point hash mismatch for ${key}`)
    }
  }
  const artifactPath = join(dirname(manifestPath), ARTIFACT_FILE)
  const compressedHash = await sha256File(artifactPath)
  if (compressedHash !== manifest.artifact.compressed_sha256) {
    throw new Error(`${artifactPath}: compressed SHA-256 mismatch`)
  }
  for (const attachment of manifest.attachments ?? []) {
    const path = join(dirname(manifestPath), attachment.file)
    const info = await stat(path)
    if (info.size !== attachment.bytes || await sha256File(path) !== attachment.sha256) {
      throw new Error(`${path}: attachment hash or size mismatch`)
    }
  }
  const attachmentNames = new Set((manifest.attachments ?? []).map(({ file }) => file))

  async function* entries() {
    const uncompressedHash = createHash('sha256')
    const hashTap = new Transform({
      transform(chunk, _encoding, callback) {
        uncompressedHash.update(chunk)
        callback(null, chunk)
      },
    })
    const input = createReadStream(artifactPath).pipe(createGunzip()).pipe(hashTap)
    const lines = createInterface({ input, crlfDelay: Infinity })
    let index = 0
    for await (const line of lines) {
      if (line.length === 0) throw new Error(`${artifactPath}: blank NDJSON line ${index + 1}`)
      let entry
      try {
        entry = JSON.parse(line)
      } catch (error) {
        throw new Error(`${artifactPath}: invalid JSON line ${index + 1}: ${error.message}`)
      }
      yield validateEntry(entry, index, `${artifactPath}:${index + 1}`)
      index += 1
    }
    if (index !== manifest.artifact.entry_count) throw new Error(`${artifactPath}: entry count mismatch`)
    if (uncompressedHash.digest('hex') !== manifest.artifact.uncompressed_sha256) {
      throw new Error(`${artifactPath}: uncompressed SHA-256 mismatch`)
    }
  }

  return {
    manifest,
    manifestPath,
    entries: entries(),
    async readAttachment(relativePath) {
      if (!attachmentNames.has(relativePath)) {
        throw new Error(`${manifestPath}: unknown attachment ${relativePath}`)
      }
      return readFile(join(dirname(manifestPath), relativePath))
    },
  }
}
