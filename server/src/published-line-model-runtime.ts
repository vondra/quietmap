//! Runtime activation boundary for the otherwise dormant popup publication IPC.

import { existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { PMTILES_BASE } from './routes/heatmap-shared.js'
import { SOURCE_READER_PATH } from './runtime-paths.js'
import { resolveTileEnv } from './tile-manifest-reader.js'
import {
  DISABLED_PUBLISHED_LINE_MODEL,
  createPublishedLineModel,
  type PublishedLineModel,
} from './published-line-model.js'

const BUNDLED_POPUP_IDENTITY = resolve(
  import.meta.dirname,
  'model-role/popup-release-identity.json',
)

/**
 * A checked-in release never chooses stock or H0 here. The immutable bundled
 * identity carries that decision. Until a later role-artifact step installs
 * it, this prep mechanism is inert and today's stock server behaves unchanged.
 */
export async function createRuntimePublishedLineModel(): Promise<PublishedLineModel> {
  const required = process.env.QM_POPUP_PUBLISH_IPC_REQUIRED
  if (required !== undefined && required !== '0' && required !== '1') {
    throw new Error('QM_POPUP_PUBLISH_IPC_REQUIRED must be 0 or 1')
  }
  if (!existsSync(BUNDLED_POPUP_IDENTITY)) {
    if (required === '1') {
      throw new Error('popup publication IPC required but immutable popup identity is absent')
    }
    return DISABLED_PUBLISHED_LINE_MODEL
  }
  const runtimeRoot = process.env.QM_RUNTIME_DIR
  if (!runtimeRoot) {
    throw new Error('immutable popup identity requires QM_RUNTIME_DIR')
  }
  // Resolve through the existing allowlist before the environment token enters
  // either the manifest name or the durable-state path.
  const tileEnv = resolveTileEnv(process.env.TILE_ENV)
  return createPublishedLineModel({
    releaseIdentityPath: BUNDLED_POPUP_IDENTITY,
    sourceReaderPath: SOURCE_READER_PATH,
    statePath: resolve(runtimeRoot, 'state', `published-line-model.${tileEnv}.json`),
    tokenPath: resolve(runtimeRoot, 'secrets', '.popup-publish-token'),
    pmtilesDir: PMTILES_BASE,
    tileEnv,
  })
}
