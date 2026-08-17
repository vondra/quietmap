//! Private immutable inputs and durable storage for popup publication transactions.

import { createHash } from 'node:crypto'
import {
  chmod, lstat, mkdir, open, readFile, rename, rm,
} from 'node:fs/promises'
import { dirname } from 'node:path'
import {
  parsePopupReleaseIdentity,
  parsePublishedLineModelState,
  type PopupReleaseIdentity,
  type PublishedLineModelState,
} from './published-line-model-contract.js'

const TOKEN = /^[0-9a-f]{64}\n$/

async function requireRegularFile(
  path: string,
  label: string,
): Promise<Awaited<ReturnType<typeof lstat>>> {
  const info = await lstat(path)
  if (!info.isFile() || info.size <= 0) {
    throw new Error(`${label} is not a non-empty regular file`)
  }
  return info
}

function requirePrivateFileMode(
  info: Awaited<ReturnType<typeof lstat>>,
  label: string,
): void {
  const currentUid = process.getuid?.()
  if ((Number(info.mode) & 0o077) !== 0 || Number(info.nlink) !== 1
      || (currentUid !== undefined && Number(info.uid) !== currentUid)) {
    throw new Error(`${label} must be a private owner-only single-link file`)
  }
}

async function sha256File(path: string): Promise<string> {
  return createHash('sha256').update(await readFile(path)).digest('hex')
}

export async function readPopupPublishToken(path: string): Promise<Buffer> {
  const info = await requireRegularFile(path, 'popup publication token')
  requirePrivateFileMode(info, 'popup publication token')
  const raw = await readFile(path, 'utf8')
  if (!TOKEN.test(raw)) throw new Error('popup publication token must be 64 lowercase hex plus LF')
  return Buffer.from(raw.trim(), 'ascii')
}

export async function readPopupReleaseIdentity(
  identityPath: string,
  sourceReaderPath: string,
): Promise<PopupReleaseIdentity> {
  await requireRegularFile(identityPath, 'popup release identity')
  await requireRegularFile(sourceReaderPath, 'source-reader addon')
  let raw: unknown
  try {
    raw = JSON.parse(await readFile(identityPath, 'utf8'))
  } catch (error) {
    throw new Error(`cannot parse popup release identity: ${(error as Error).message}`)
  }
  const identity = parsePopupReleaseIdentity(raw)
  if (await sha256File(sourceReaderPath) !== identity.native_addon_sha256) {
    throw new Error('source-reader addon differs from the immutable popup release identity')
  }
  return identity
}

export async function writePublishedLineModelState(
  path: string,
  state: PublishedLineModelState,
): Promise<void> {
  const parent = dirname(path)
  await mkdir(parent, { recursive: true, mode: 0o700 })
  // mkdir does not narrow an existing directory. State must remain private
  // even when an operator or older release created the parent too broadly.
  await chmod(parent, 0o700)
  const temporary = `${path}.tmp-${process.pid}-${Date.now()}`
  const handle = await open(temporary, 'wx', 0o600)
  try {
    await handle.writeFile(`${JSON.stringify(state)}\n`)
    await handle.sync()
  } finally {
    await handle.close()
  }
  try {
    await rename(temporary, path)
    // A directory-sync failure can mean the rename reached the live filesystem.
    // The caller still publishes nothing in memory; restart sees the durable
    // prepared state and remains unavailable until the transaction is resolved.
    const directory = await open(parent, 'r')
    try { await directory.sync() } finally { await directory.close() }
  } finally {
    await rm(temporary, { force: true })
  }
}

export async function readPublishedLineModelState(
  path: string,
): Promise<PublishedLineModelState | null> {
  try {
    const info = await requireRegularFile(path, 'published line-model state')
    requirePrivateFileMode(info, 'published line-model state')
    return parsePublishedLineModelState(JSON.parse(await readFile(path, 'utf8')))
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return null
    throw error
  }
}
