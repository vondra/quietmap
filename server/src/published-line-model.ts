//! Process-wide map/popup model snapshot with durable two-phase publication state.

import { timingSafeEqual } from 'node:crypto'
import { readFile } from 'node:fs/promises'
import {
  POPUP_PUBLISH_IPC_SCHEMA,
  parseCommitRequest,
  parsePrepareRequest,
  type CommittedPublishedLineModelState,
  type PopupPublishCommitRequest,
  type PopupPublishPrepareRequest,
  type PopupReleaseIdentity,
  type PreparedPublishedLineModelState,
  type PublishedLineModelState,
  type PublishedManifestSnapshot,
} from './published-line-model-contract.js'
import {
  type PublishedLineModel,
  type PublishedLineModelAck,
  type PublishedLineModelOptions,
  PublishedLineModelUnavailableError,
} from './published-line-model-api.js'
import {
  publishedManifestSnapshotFromText,
  publishedManifestSnapshotsEqual,
} from './published-line-model-snapshot.js'
import {
  readPopupPublishToken,
  readPopupReleaseIdentity,
  readPublishedLineModelState,
  writePublishedLineModelState,
} from './published-line-model-store.js'
import {
  type PmtilesManifest,
  validatePmtilesManifest,
} from './runtime-readiness.js'
import { resolveManifestPath } from './tile-manifest-reader.js'

export {
  DISABLED_PUBLISHED_LINE_MODEL,
  PublishedLineModelUnavailableError,
  type PublishedLineModel,
  type PublishedLineModelAck,
  type PublishedLineModelOptions,
} from './published-line-model-api.js'

class ActivePublishedLineModel implements PublishedLineModel {
  readonly enabled = true
  private readonly options: PublishedLineModelOptions
  private readonly releaseIdentity: PopupReleaseIdentity
  private readonly token: Buffer
  private persistentState: PublishedLineModelState | null = null
  private stateError: string | null = null
  private stateCorrupt = false
  private mapSnapshot: PublishedManifestSnapshot | null = null
  private popupAvailable = false
  private operationTail: Promise<void> = Promise.resolve()

  private constructor(
    options: PublishedLineModelOptions,
    releaseIdentity: PopupReleaseIdentity,
    token: Buffer,
  ) {
    this.options = options
    this.releaseIdentity = releaseIdentity
    this.token = token
  }

  static async create(options: PublishedLineModelOptions): Promise<ActivePublishedLineModel> {
    const [releaseIdentity, token] = await Promise.all([
      readPopupReleaseIdentity(options.releaseIdentityPath, options.sourceReaderPath),
      readPopupPublishToken(options.tokenPath),
    ])
    const manager = new ActivePublishedLineModel(options, releaseIdentity, token)
    await manager.initialize()
    return manager
  }

  private async initialize(): Promise<void> {
    try {
      this.persistentState = await readPublishedLineModelState(this.options.statePath)
      if (!this.persistentState) {
        this.stateError = 'published line-model state is absent; bootstrap PREPARE/COMMIT required'
        return
      }
      if (this.persistentState.phase === 'prepared') {
        await this.validateStoredSnapshot(this.persistentState.previous, 'prepared previous manifest')
        await this.validateStoredSnapshot(this.persistentState.next, 'prepared next manifest')
        const disk = await this.readCurrentManifest()
        if (!publishedManifestSnapshotsEqual(disk, this.persistentState.previous)
            && !publishedManifestSnapshotsEqual(disk, this.persistentState.next)) {
          throw new Error('current manifest is neither side of the prepared transaction')
        }
        this.mapSnapshot = this.persistentState.previous
        this.popupAvailable = false
        return
      }
      await this.validateStoredSnapshot(this.persistentState.current, 'committed manifest')
      const disk = await this.readCurrentManifest()
      if (!publishedManifestSnapshotsEqual(disk, this.persistentState.current)) {
        throw new Error('current manifest changed without a PREPARE/COMMIT transaction')
      }
      this.mapSnapshot = this.persistentState.current
      this.popupAvailable = this.releaseMatches(this.persistentState.current)
      if (!this.popupAvailable) {
        throw new Error('popup release identity differs from the committed line model')
      }
    } catch (error) {
      this.stateError = error instanceof Error ? error.message : String(error)
      this.stateCorrupt = true
      // A corrupt/unverifiable committed role must not leave a previously
      // assigned map snapshot live. Map and popup recover together via a later
      // valid publication transaction instead of serving mixed physics.
      this.mapSnapshot = null
      this.popupAvailable = false
    }
  }

  authenticate(value: unknown): boolean {
    if (typeof value !== 'string') return false
    const candidate = Buffer.from(value, 'ascii')
    return candidate.length === this.token.length && timingSafeEqual(candidate, this.token)
  }

  assertPopupAvailable(): void {
    if (!this.popupAvailable || !this.mapSnapshot || !this.releaseMatches(this.mapSnapshot)) {
      throw new PublishedLineModelUnavailableError(
        `published line-model snapshot unavailable: ${this.stateError ?? 'publication prepared'}`,
      )
    }
  }

  assertReady(): void {
    this.assertPopupAvailable()
  }

  mapManifest(): PmtilesManifest | null {
    return this.mapSnapshot?.manifest ?? null
  }

  async prepare(value: unknown): Promise<PublishedLineModelAck> {
    const request = parsePrepareRequest(value)
    return this.serialized(() => this.prepareLocked(request))
  }

  async commit(value: unknown): Promise<PublishedLineModelAck> {
    const request = parseCommitRequest(value)
    return this.serialized(() => this.commitLocked(request))
  }

  private async prepareLocked(request: PopupPublishPrepareRequest): Promise<PublishedLineModelAck> {
    if (this.stateCorrupt || (this.stateError && this.persistentState !== null)) {
      throw new Error(`persistent state is invalid: ${this.stateError}`)
    }
    const current = await this.readCurrentManifest()
    if (current.manifest_sha256 !== request.previous_manifest_sha256) {
      throw new Error('PREPARE previous manifest hash differs from the live pointer')
    }
    if (!this.releaseMatches(current)) {
      throw new Error('PREPARE active popup release differs from the live line model')
    }
    const next = await publishedManifestSnapshotFromText(
      request.next_manifest_text,
      this.options.pmtilesDir,
      'PREPARE next manifest',
    )
    if (this.persistentState?.phase === 'prepared') {
      if (this.persistentState.transaction_id === request.transaction_id
          && publishedManifestSnapshotsEqual(this.persistentState.previous, current)
          && publishedManifestSnapshotsEqual(this.persistentState.next, next)) {
        return this.ack(this.persistentState, this.persistentState.previous, true)
      }
      throw new Error('another popup publication transaction is already prepared')
    }
    if (this.persistentState?.phase === 'committed'
        && !publishedManifestSnapshotsEqual(this.persistentState.current, current)) {
      throw new Error('committed state differs from the live pointer')
    }
    const prepared: PreparedPublishedLineModelState = {
      schema: POPUP_PUBLISH_IPC_SCHEMA,
      phase: 'prepared',
      transaction_id: request.transaction_id,
      previous: current,
      next,
    }
    await writePublishedLineModelState(this.options.statePath, prepared)
    this.persistentState = prepared
    this.mapSnapshot = current
    this.popupAvailable = false
    this.stateError = null
    this.stateCorrupt = false
    return this.ack(prepared, current, false)
  }

  private async commitLocked(request: PopupPublishCommitRequest): Promise<PublishedLineModelAck> {
    if (this.stateError) throw new Error(`persistent state is invalid: ${this.stateError}`)
    if (this.persistentState?.phase === 'committed') {
      if (this.persistentState.transaction_id !== request.transaction_id) {
        throw new Error('COMMIT transaction differs from the committed transaction')
      }
      if (this.persistentState.result !== request.result) {
        throw new Error('idempotent COMMIT result differs from the committed result')
      }
      const disk = await this.readCurrentManifest()
      if (!publishedManifestSnapshotsEqual(disk, this.persistentState.current)
          || !this.releaseMatches(disk)) {
        this.popupAvailable = false
        throw new Error('idempotent COMMIT no longer matches disk or popup release identity')
      }
      this.mapSnapshot = disk
      this.popupAvailable = true
      return this.ack(this.persistentState, disk, true)
    }
    if (!this.persistentState || this.persistentState.transaction_id !== request.transaction_id) {
      throw new Error('COMMIT has no matching prepared transaction')
    }
    const expected = request.result === 'next'
      ? this.persistentState.next
      : this.persistentState.previous
    const disk = await this.readCurrentManifest()
    if (!publishedManifestSnapshotsEqual(disk, expected)) {
      throw new Error(`COMMIT ${request.result} manifest differs from the live pointer`)
    }
    if (!this.releaseMatches(disk)) {
      throw new Error('COMMIT popup release differs from the live line model')
    }
    const committed: CommittedPublishedLineModelState = {
      schema: POPUP_PUBLISH_IPC_SCHEMA,
      phase: 'committed',
      transaction_id: request.transaction_id,
      result: request.result,
      current: disk,
    }
    await writePublishedLineModelState(this.options.statePath, committed)
    this.persistentState = committed
    this.mapSnapshot = disk
    this.popupAvailable = true
    return this.ack(committed, disk, false)
  }

  private async validateStoredSnapshot(snapshot: PublishedManifestSnapshot, label: string): Promise<void> {
    await validatePmtilesManifest(snapshot.manifest, this.options.pmtilesDir, label)
  }

  private async readCurrentManifest(): Promise<PublishedManifestSnapshot> {
    const path = resolveManifestPath(this.options.pmtilesDir, this.options.tileEnv)
    return publishedManifestSnapshotFromText(
      await readFile(path, 'utf8'),
      this.options.pmtilesDir,
      path,
    )
  }

  private releaseMatches(snapshot: PublishedManifestSnapshot): boolean {
    return this.releaseIdentity.line_model_role_sha256 === snapshot.line_model_role_sha256
  }

  private ack(
    state: PublishedLineModelState,
    snapshot: PublishedManifestSnapshot,
    idempotent: boolean,
  ): PublishedLineModelAck {
    return {
      schema: POPUP_PUBLISH_IPC_SCHEMA,
      phase: state.phase,
      transaction_id: state.transaction_id,
      manifest_sha256: snapshot.manifest_sha256,
      line_model_role_sha256: snapshot.line_model_role_sha256,
      build: snapshot.build,
      idempotent,
    }
  }

  private async serialized<T>(operation: () => Promise<T>): Promise<T> {
    const before = this.operationTail
    let release!: () => void
    this.operationTail = new Promise<void>((resolve) => { release = resolve })
    await before
    try { return await operation() } finally { release() }
  }
}

export async function createPublishedLineModel(
  options: PublishedLineModelOptions,
): Promise<PublishedLineModel> {
  return ActivePublishedLineModel.create(options)
}
