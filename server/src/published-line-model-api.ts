//! Public API and dormant implementation for the process-wide published line model.

import type { PmtilesManifest } from './runtime-readiness.js'

export class PublishedLineModelUnavailableError extends Error {
  readonly code = 'QM_PUBLISHED_LINE_MODEL_UNAVAILABLE'
}

export type PublishedLineModelAck = {
  schema: 1
  phase: 'prepared' | 'committed'
  transaction_id: string
  manifest_sha256: string
  line_model_role_sha256: string
  build: string
  idempotent: boolean
}

export type PublishedLineModel = {
  readonly enabled: boolean
  authenticate(token: unknown): boolean
  assertPopupAvailable(): void
  assertReady(): void
  mapManifest(): PmtilesManifest | null
  prepare(value: unknown): Promise<PublishedLineModelAck>
  commit(value: unknown): Promise<PublishedLineModelAck>
}

export type PublishedLineModelOptions = {
  releaseIdentityPath: string
  sourceReaderPath: string
  statePath: string
  tokenPath: string
  pmtilesDir: string
  tileEnv?: string
}

class DisabledPublishedLineModel implements PublishedLineModel {
  readonly enabled = false
  authenticate(): boolean { return false }
  assertPopupAvailable(): void {}
  assertReady(): void {}
  mapManifest(): PmtilesManifest | null { return null }
  async prepare(): Promise<PublishedLineModelAck> {
    throw new PublishedLineModelUnavailableError('popup publication IPC is disabled')
  }
  async commit(): Promise<PublishedLineModelAck> {
    throw new PublishedLineModelUnavailableError('popup publication IPC is disabled')
  }
}

export const DISABLED_PUBLISHED_LINE_MODEL: PublishedLineModel =
  new DisabledPublishedLineModel()
