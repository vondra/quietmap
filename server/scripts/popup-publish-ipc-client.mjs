#!/usr/bin/env node
//! Local authenticated client for the popup publication PREPARE/ACK/COMMIT protocol.

import { createHash } from 'node:crypto'
import { lstat, readFile } from 'node:fs/promises'
import { pathToFileURL } from 'node:url'
import { resolve } from 'node:path'

const SHA256 = /^[0-9a-f]{64}$/
const TOKEN = /^[0-9a-f]{64}\n$/

export class PopupPublishClientUsageError extends Error {}

function usage() {
  throw new PopupPublishClientUsageError(
    'usage: popup-publish-ipc-client.mjs prepare <loopback-base-url> <token-file> '
    + '<transaction-id> <previous-manifest> <next-manifest>\n'
    + '   or: popup-publish-ipc-client.mjs commit <loopback-base-url> <token-file> '
    + '<transaction-id> <next|previous>',
  )
}

function requireLoopbackBaseUrl(text) {
  const url = new URL(text)
  if (url.protocol !== 'http:'
      || !['127.0.0.1', '[::1]', '::1'].includes(url.hostname)
      || url.username || url.password || url.pathname !== '/' || url.search || url.hash) {
    throw new Error('popup publication base URL must be an uncredentialed loopback http origin')
  }
  return url
}

async function readPrivateToken(path) {
  const info = await lstat(path)
  const currentUid = process.getuid?.()
  if (!info.isFile() || info.size <= 0
      || (info.mode & 0o077) !== 0 || info.nlink !== 1
      || (currentUid !== undefined && info.uid !== currentUid)) {
    throw new Error('popup publication token must be a private owner-only single-link regular file')
  }
  const raw = await readFile(path, 'utf8')
  if (!TOKEN.test(raw)) throw new Error('popup publication token has an invalid format')
  return raw.trim()
}

function manifestText(path, text) {
  let parsed
  try { parsed = JSON.parse(text) } catch (error) {
    throw new Error(`${path} is not JSON: ${error.message}`)
  }
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)
      || !SHA256.test(parsed.line_model_role_sha256 ?? '')) {
    throw new Error(`${path} has no line_model_role_sha256`)
  }
  return text
}

async function post(base, phase, token, body, fetchImpl, stdout) {
  const url = new URL(`/api/internal/popup-publish/${phase}`, base)
  const response = await fetchImpl(url, {
    method: 'POST',
    redirect: 'error',
    signal: AbortSignal.timeout(15_000),
    headers: {
      'content-type': 'application/json',
      'x-qm-popup-publish-token': token,
    },
    body: JSON.stringify(body),
  })
  const text = await response.text()
  if (!response.ok) {
    throw new Error(`popup publication ${phase.toUpperCase()} failed HTTP ${response.status}: ${text}`)
  }
  const reply = JSON.parse(text)
  if (reply.schema !== 1 || reply.phase !== (phase === 'prepare' ? 'prepared' : 'committed')
      || reply.transaction_id !== body.transaction_id || !SHA256.test(reply.manifest_sha256 ?? '')
      || !SHA256.test(reply.line_model_role_sha256 ?? '')) {
    throw new Error(`popup publication ${phase.toUpperCase()} returned an invalid ACK`)
  }
  stdout.write(`${JSON.stringify(reply)}\n`)
}

export async function runPopupPublishIpcClient(
  argv,
  { fetchImpl = fetch, stdout = process.stdout } = {},
) {
  const [command, baseText, tokenPath, transactionId, ...rest] = argv
  if (!['prepare', 'commit'].includes(command) || !baseText || !tokenPath
      || !SHA256.test(transactionId ?? '')) usage()
  const [base, token] = await Promise.all([
    Promise.resolve(requireLoopbackBaseUrl(baseText)),
    readPrivateToken(tokenPath),
  ])
  if (command === 'prepare') {
    if (rest.length !== 2) usage()
    const [previousPath, nextPath] = rest
    const [previousText, nextText] = await Promise.all([
      readFile(previousPath, 'utf8'),
      readFile(nextPath, 'utf8'),
    ])
    manifestText(previousPath, previousText)
    await post(base, command, token, {
      schema: 1,
      transaction_id: transactionId,
      previous_manifest_sha256: createHash('sha256').update(previousText).digest('hex'),
      next_manifest_text: manifestText(nextPath, nextText),
    }, fetchImpl, stdout)
    return
  }
  if (rest.length !== 1 || !['next', 'previous'].includes(rest[0])) usage()
  await post(base, command, token, {
    schema: 1,
    transaction_id: transactionId,
    result: rest[0],
  }, fetchImpl, stdout)
}

export async function main(argv = process.argv.slice(2)) {
  await runPopupPublishIpcClient(argv)
}

const isMain = process.argv[1]
  && import.meta.url === pathToFileURL(resolve(process.argv[1])).href
if (isMain) {
  try { await main() } catch (error) {
    console.error(`popup-publish-ipc-client: FAIL: ${error.message}`)
    process.exitCode = error instanceof PopupPublishClientUsageError ? 2 : 1
  }
}
