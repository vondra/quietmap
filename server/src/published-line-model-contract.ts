//! Pure schemas for the process-wide published line-model identity and its IPC state.

import { createHash } from 'node:crypto'
import type { PmtilesManifest } from './runtime-readiness.js'

const SHA256 = /^[0-9a-f]{64}$/
const IDENTIFIER = /^[a-z][a-z0-9-]*$/
const BUILD_ID = /^b[0-9]+$/
export const MAX_POPUP_PUBLISH_MANIFEST_TEXT_BYTES = 5 * 1024 * 1024

export const POPUP_PUBLISH_IPC_SCHEMA = 1 as const

export type PopupReleaseIdentity = {
  schema: 1
  artifact_family: 'popup-production'
  resolved_role: string
  model_role: 'stock' | 'h0'
  selection_epoch: number | null
  line_model_role_sha256: string
  artifact_manifest_sha256: string
  native_addon_sha256: string
}

export type PublishedManifestSnapshot = {
  build: string
  line_model_role_sha256: string
  manifest_sha256: string
  manifest_text: string
  manifest: PmtilesManifest
}

export type PreparedPublishedLineModelState = {
  schema: 1
  phase: 'prepared'
  transaction_id: string
  previous: PublishedManifestSnapshot
  next: PublishedManifestSnapshot
}

export type CommittedPublishedLineModelState = {
  schema: 1
  phase: 'committed'
  transaction_id: string
  result: 'next' | 'previous'
  current: PublishedManifestSnapshot
}

export type PublishedLineModelState =
  | PreparedPublishedLineModelState
  | CommittedPublishedLineModelState

export type PopupPublishPrepareRequest = {
  schema: 1
  transaction_id: string
  previous_manifest_sha256: string
  next_manifest_text: string
}

export type PopupPublishCommitRequest = {
  schema: 1
  transaction_id: string
  result: 'next' | 'previous'
}

function exactKeys(value: Record<string, unknown>, expected: string[], label: string): void {
  const actual = Object.keys(value).sort()
  const wanted = [...expected].sort()
  if (actual.join('\0') !== wanted.join('\0')) {
    throw new Error(`${label} has missing or unexpected fields`)
  }
}

function object(value: unknown, label: string): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error(`${label} must be an object`)
  }
  return value as Record<string, unknown>
}

function sha256(value: unknown, label: string): string {
  if (typeof value !== 'string' || !SHA256.test(value)) {
    throw new Error(`${label} must be a lowercase SHA-256`)
  }
  return value
}

function identifier(value: unknown, label: string): string {
  if (typeof value !== 'string' || !IDENTIFIER.test(value)) {
    throw new Error(`${label} must be a canonical identifier`)
  }
  return value
}

export function sha256Text(text: string): string {
  return createHash('sha256').update(text).digest('hex')
}

export function parsePopupReleaseIdentity(value: unknown): PopupReleaseIdentity {
  const raw = object(value, 'popup release identity')
  exactKeys(raw, [
    'artifact_family', 'artifact_manifest_sha256', 'line_model_role_sha256', 'model_role',
    'native_addon_sha256', 'resolved_role', 'schema', 'selection_epoch',
  ], 'popup release identity')
  if (raw.schema !== POPUP_PUBLISH_IPC_SCHEMA || raw.artifact_family !== 'popup-production') {
    throw new Error('popup release identity has the wrong schema or family')
  }
  const modelRole = raw.model_role
  if (modelRole !== 'stock' && modelRole !== 'h0') {
    throw new Error('popup release identity has an invalid model role')
  }
  const selectionEpoch = raw.selection_epoch
  if (modelRole === 'stock') {
    if (selectionEpoch !== null) throw new Error('stock popup release must not carry an epoch')
  } else if (!Number.isSafeInteger(selectionEpoch) || (selectionEpoch as number) <= 0) {
    throw new Error('H0 popup release must carry a positive selection epoch')
  }
  return {
    schema: POPUP_PUBLISH_IPC_SCHEMA,
    artifact_family: 'popup-production',
    resolved_role: identifier(raw.resolved_role, 'resolved_role'),
    model_role: modelRole,
    selection_epoch: selectionEpoch as number | null,
    line_model_role_sha256: sha256(raw.line_model_role_sha256, 'line_model_role_sha256'),
    artifact_manifest_sha256: sha256(raw.artifact_manifest_sha256, 'artifact_manifest_sha256'),
    native_addon_sha256: sha256(raw.native_addon_sha256, 'native_addon_sha256'),
  }
}

export function parseManifestSnapshot(value: unknown, label: string): PublishedManifestSnapshot {
  const raw = object(value, label)
  exactKeys(raw, [
    'build', 'line_model_role_sha256', 'manifest', 'manifest_sha256', 'manifest_text',
  ], label)
  if (typeof raw.build !== 'string' || !BUILD_ID.test(raw.build)) {
    throw new Error(`${label}.build is invalid`)
  }
  if (typeof raw.manifest_text !== 'string' || raw.manifest_text.length === 0) {
    throw new Error(`${label}.manifest_text is empty`)
  }
  const manifestSha = sha256(raw.manifest_sha256, `${label}.manifest_sha256`)
  if (sha256Text(raw.manifest_text) !== manifestSha) {
    throw new Error(`${label}.manifest_text hash mismatch`)
  }
  let parsed: unknown
  try {
    parsed = JSON.parse(raw.manifest_text)
  } catch (error) {
    throw new Error(`${label}.manifest_text is not JSON: ${(error as Error).message}`)
  }
  if (JSON.stringify(parsed) !== JSON.stringify(raw.manifest)) {
    throw new Error(`${label}.manifest differs from manifest_text`)
  }
  const manifest = object(parsed, `${label}.manifest`) as PmtilesManifest
  const lineDigest = sha256(raw.line_model_role_sha256, `${label}.line_model_role_sha256`)
  if (manifest.build !== raw.build || manifest.line_model_role_sha256 !== lineDigest) {
    throw new Error(`${label} identity differs from its manifest`)
  }
  return {
    build: raw.build,
    line_model_role_sha256: lineDigest,
    manifest_sha256: manifestSha,
    manifest_text: raw.manifest_text,
    manifest,
  }
}

export function parsePublishedLineModelState(value: unknown): PublishedLineModelState {
  const raw = object(value, 'published line-model state')
  const transactionId = sha256(raw.transaction_id, 'transaction_id')
  if (raw.schema !== POPUP_PUBLISH_IPC_SCHEMA) {
    throw new Error('published line-model state has the wrong schema')
  }
  if (raw.phase === 'prepared') {
    exactKeys(raw, ['next', 'phase', 'previous', 'schema', 'transaction_id'], 'prepared state')
    return {
      schema: POPUP_PUBLISH_IPC_SCHEMA,
      phase: 'prepared',
      transaction_id: transactionId,
      previous: parseManifestSnapshot(raw.previous, 'prepared.previous'),
      next: parseManifestSnapshot(raw.next, 'prepared.next'),
    }
  }
  if (raw.phase === 'committed') {
    exactKeys(raw, ['current', 'phase', 'result', 'schema', 'transaction_id'], 'committed state')
    if (raw.result !== 'next' && raw.result !== 'previous') {
      throw new Error('committed state has an invalid result')
    }
    return {
      schema: POPUP_PUBLISH_IPC_SCHEMA,
      phase: 'committed',
      transaction_id: transactionId,
      result: raw.result,
      current: parseManifestSnapshot(raw.current, 'committed.current'),
    }
  }
  throw new Error('published line-model state has an invalid phase')
}

export function parsePrepareRequest(value: unknown): PopupPublishPrepareRequest {
  const raw = object(value, 'PREPARE request')
  exactKeys(raw, [
    'next_manifest_text', 'previous_manifest_sha256', 'schema', 'transaction_id',
  ], 'PREPARE request')
  if (raw.schema !== POPUP_PUBLISH_IPC_SCHEMA) throw new Error('PREPARE schema must be 1')
  if (typeof raw.next_manifest_text !== 'string' || raw.next_manifest_text.length === 0) {
    throw new Error('PREPARE next_manifest_text is empty')
  }
  if (Buffer.byteLength(raw.next_manifest_text) > MAX_POPUP_PUBLISH_MANIFEST_TEXT_BYTES) {
    throw new Error('PREPARE next_manifest_text exceeds the publication IPC limit')
  }
  return {
    schema: POPUP_PUBLISH_IPC_SCHEMA,
    transaction_id: sha256(raw.transaction_id, 'PREPARE transaction_id'),
    previous_manifest_sha256: sha256(
      raw.previous_manifest_sha256,
      'PREPARE previous_manifest_sha256',
    ),
    next_manifest_text: raw.next_manifest_text,
  }
}

export function parseCommitRequest(value: unknown): PopupPublishCommitRequest {
  const raw = object(value, 'COMMIT request')
  exactKeys(raw, ['result', 'schema', 'transaction_id'], 'COMMIT request')
  if (raw.schema !== POPUP_PUBLISH_IPC_SCHEMA) throw new Error('COMMIT schema must be 1')
  if (raw.result !== 'next' && raw.result !== 'previous') {
    throw new Error('COMMIT result must be next or previous')
  }
  return {
    schema: POPUP_PUBLISH_IPC_SCHEMA,
    transaction_id: sha256(raw.transaction_id, 'COMMIT transaction_id'),
    result: raw.result,
  }
}
