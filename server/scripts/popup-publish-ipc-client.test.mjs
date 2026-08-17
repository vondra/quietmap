//! Hermetic tests for the local popup publication IPC client.

import assert from 'node:assert/strict'
import { chmod, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { mkdtemp } from 'node:fs/promises'
import test from 'node:test'

import {
  PopupPublishClientUsageError,
  runPopupPublishIpcClient,
} from './popup-publish-ipc-client.mjs'

const TRANSACTION_ID = '11'.repeat(32)
const PREVIOUS_LINE_MODEL = '22'.repeat(32)
const NEXT_LINE_MODEL = '33'.repeat(32)
const TOKEN = '44'.repeat(32)

function manifest(lineModelRoleSha256, buildId) {
  return `${JSON.stringify({
    schema: 1,
    build_id: buildId,
    line_model_role_sha256: lineModelRoleSha256,
  })}\n`
}

async function fixture() {
  const root = await mkdtemp(join(tmpdir(), 'popup-publish-client-'))
  const tokenPath = join(root, 'token')
  const previousPath = join(root, 'previous.json')
  const nextPath = join(root, 'next.json')
  await Promise.all([
    writeFile(tokenPath, `${TOKEN}\n`, { mode: 0o600 }),
    writeFile(previousPath, manifest(PREVIOUS_LINE_MODEL, 'previous')),
    writeFile(nextPath, manifest(NEXT_LINE_MODEL, 'next')),
  ])
  return { root, tokenPath, previousPath, nextPath }
}

function acknowledgedFetch(calls, phase, lineModelRoleSha256) {
  return async (url, options) => {
    calls.push({ url: url.toString(), options })
    const body = JSON.parse(options.body)
    return new Response(JSON.stringify({
      schema: 1,
      phase,
      transaction_id: body.transaction_id,
      manifest_sha256: '55'.repeat(32),
      line_model_role_sha256: lineModelRoleSha256,
    }), { status: 200 })
  }
}

test('prepare sends exact manifests and accepts a bound ACK', async (t) => {
  const { root, tokenPath, previousPath, nextPath } = await fixture()
  t.after(async () => rm(root, { recursive: true, force: true }))
  const calls = []
  const output = []
  await runPopupPublishIpcClient([
    'prepare', 'http://127.0.0.1:8531/', tokenPath, TRANSACTION_ID,
    previousPath, nextPath,
  ], {
    fetchImpl: acknowledgedFetch(calls, 'prepared', NEXT_LINE_MODEL),
    stdout: { write: (value) => output.push(value) },
  })

  assert.equal(calls.length, 1)
  assert.equal(calls[0].url, 'http://127.0.0.1:8531/api/internal/popup-publish/prepare')
  assert.equal(calls[0].options.headers['x-qm-popup-publish-token'], TOKEN)
  const body = JSON.parse(calls[0].options.body)
  assert.equal(body.transaction_id, TRANSACTION_ID)
  assert.match(body.previous_manifest_sha256, /^[0-9a-f]{64}$/)
  assert.equal(body.next_manifest_text, manifest(NEXT_LINE_MODEL, 'next'))
  assert.equal(JSON.parse(output.join('')).phase, 'prepared')
})

test('commit binds result and transaction to the ACK', async (t) => {
  const { root, tokenPath } = await fixture()
  t.after(async () => rm(root, { recursive: true, force: true }))
  const calls = []
  await runPopupPublishIpcClient([
    'commit', 'http://[::1]:8531/', tokenPath, TRANSACTION_ID, 'previous',
  ], {
    fetchImpl: acknowledgedFetch(calls, 'committed', PREVIOUS_LINE_MODEL),
    stdout: { write: () => {} },
  })
  assert.deepEqual(JSON.parse(calls[0].options.body), {
    schema: 1,
    transaction_id: TRANSACTION_ID,
    result: 'previous',
  })
})

test('rejects a non-loopback endpoint before fetch', async (t) => {
  const { root, tokenPath } = await fixture()
  t.after(async () => rm(root, { recursive: true, force: true }))
  let fetched = false
  await assert.rejects(
    runPopupPublishIpcClient([
      'commit', 'http://example.test/', tokenPath, TRANSACTION_ID, 'next',
    ], { fetchImpl: async () => { fetched = true } }),
    /loopback/,
  )
  assert.equal(fetched, false)
})

test('rejects an owner-readable-by-group token', async (t) => {
  const { root, tokenPath } = await fixture()
  t.after(async () => rm(root, { recursive: true, force: true }))
  await chmod(tokenPath, 0o640)
  await assert.rejects(
    runPopupPublishIpcClient([
      'commit', 'http://127.0.0.1:8531/', tokenPath, TRANSACTION_ID, 'next',
    ]),
    /private owner-only/,
  )
})

test('rejects an ACK for another transaction', async (t) => {
  const { root, tokenPath } = await fixture()
  t.after(async () => rm(root, { recursive: true, force: true }))
  await assert.rejects(
    runPopupPublishIpcClient([
      'commit', 'http://127.0.0.1:8531/', tokenPath, TRANSACTION_ID, 'next',
    ], {
      fetchImpl: async () => new Response(JSON.stringify({
        schema: 1,
        phase: 'committed',
        transaction_id: '66'.repeat(32),
        manifest_sha256: '77'.repeat(32),
        line_model_role_sha256: NEXT_LINE_MODEL,
      }), { status: 200 }),
    }),
    /invalid ACK/,
  )
})

test('usage errors are distinguishable by the CLI wrapper', async () => {
  await assert.rejects(
    runPopupPublishIpcClient(['commit']),
    PopupPublishClientUsageError,
  )
})
