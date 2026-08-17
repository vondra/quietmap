//! Validation and exact identity comparison for published PMTiles manifest snapshots.

import {
  sha256Text,
  type PublishedManifestSnapshot,
} from './published-line-model-contract.js'
import {
  type PmtilesManifest,
  validatePmtilesManifest,
} from './runtime-readiness.js'

export async function publishedManifestSnapshotFromText(
  text: string,
  pmtilesDir: string,
  label: string,
): Promise<PublishedManifestSnapshot> {
  let manifest: PmtilesManifest
  try {
    manifest = JSON.parse(text) as PmtilesManifest
  } catch (error) {
    throw new Error(`${label} is not JSON: ${(error as Error).message}`)
  }
  await validatePmtilesManifest(manifest, pmtilesDir, label)
  if (typeof manifest.line_model_role_sha256 !== 'string'
      || !/^[0-9a-f]{64}$/.test(manifest.line_model_role_sha256)) {
    throw new Error(`${label} has no valid line_model_role_sha256`)
  }
  if (typeof manifest.build !== 'string') throw new Error(`${label} has no build`)
  return {
    build: manifest.build,
    line_model_role_sha256: manifest.line_model_role_sha256,
    manifest_sha256: sha256Text(text),
    manifest_text: text,
    manifest,
  }
}

export function publishedManifestSnapshotsEqual(
  a: PublishedManifestSnapshot,
  b: PublishedManifestSnapshot,
): boolean {
  return a.manifest_sha256 === b.manifest_sha256
    && a.line_model_role_sha256 === b.line_model_role_sha256
    && a.build === b.build
}
