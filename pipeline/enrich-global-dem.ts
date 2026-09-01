/**
 * Global DEM enrichment: Copernicus GLO-30 → data/prepared/dem/copernicus/*.hgt.
 *
 * Replaces the SRTM (2000) baseline with Copernicus GLO-30 (2021). Same 30 m
 * resolution, newer source, fewer void artefacts, better coverage > 60 °N.
 * The raster-reader already prefers dem/copernicus/ over dem/srtm/ (fallback),
 * so dropping newer .hgt tiles in-place is all that is required.
 *
 * This script is a thin orchestrator over two existing bash helpers:
 *   - scripts/rasters/download-copernicus-dem.sh (~1.26 TB from s3://copernicus-dem-30m)
 *   - scripts/rasters/convert-dem-copernicus.sh (VRT warp → 3601×3601 i16 BE .hgt)
 *
 * Usage:
 *   cd pipeline && npx tsx enrich-global-dem.ts            # convert only (assume source present)
 *   cd pipeline && npx tsx enrich-global-dem.ts --download # also download from S3 first
 */
import { existsSync, mkdirSync, readdirSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { execSync, spawnSync } from 'node:child_process'
import { SOURCES_BY_KEY } from './lib/sources.js'

const ROOT = resolve(import.meta.dirname, '..')
const SRC_DIR = resolve(ROOT, 'data/source/dem/copernicus-glo30')
const DST_DIR = resolve(ROOT, 'data/prepared/dem/copernicus')
const CACHE_DIR = resolve(ROOT, 'data/enrichment/global/copernicus-dem')
const DOWNLOAD_SH = resolve(ROOT, 'scripts/rasters/download-copernicus-dem.sh')
const CONVERT_SH = resolve(ROOT, 'scripts/rasters/convert-dem-copernicus.sh')

const DATASET = SOURCES_BY_KEY.get('global-copernicus-glo30')!
const doDownload = process.argv.includes('--download')

function countTiles(dir: string, ext: string): number {
  if (!existsSync(dir)) return 0
  return readdirSync(dir).filter(f => f.endsWith(ext)).length
}

function run(cmd: string, args: string[]): void {
  const label = `${cmd} ${args.join(' ')}`
  console.log(`  $ ${label}`)
  const res = spawnSync(cmd, args, { stdio: 'inherit', cwd: ROOT })
  if (res.status !== 0) {
    throw new Error(`Command failed (${res.status}): ${label}`)
  }
}

async function main(): Promise<void> {
  console.log('=== Global DEM Enrichment (Copernicus GLO-30) ===\n')
  console.log(`  Dataset:    ${DATASET.name} (id=${DATASET.id}, ${DATASET.provenance})`)
  console.log(`  Source:     ${SRC_DIR}`)
  console.log(`  Destination: ${DST_DIR}`)
  console.log(`  Existing .hgt tiles: ${countTiles(DST_DIR, '.hgt')}\n`)

  const srcReady = existsSync(SRC_DIR) && readdirSync(SRC_DIR).length > 0
  if (!srcReady) {
    if (!doDownload) {
      console.error(`ERROR: source tree empty or missing: ${SRC_DIR}`)
      console.error(`  Pass --download to fetch ~1.26 TB from s3://copernicus-dem-30m/ (4–8 h)`)
      process.exit(1)
    }
    console.log('--- Step 1: Download Copernicus GLO-30 from AWS ---')
    run('bash', [DOWNLOAD_SH])
  } else if (doDownload) {
    console.log('--- Step 1: Re-syncing Copernicus GLO-30 from AWS (--download) ---')
    run('bash', [DOWNLOAD_SH])
  } else {
    console.log('  Source tree present — skipping download (use --download to refresh)')
  }

  console.log('\n--- Step 2: Convert COG → 3601×3601 i16 BE .hgt ---')
  run('bash', [CONVERT_SH])

  const done = countTiles(DST_DIR, '.hgt')
  console.log(`\n  Converted tiles: ${done}`)

  mkdirSync(CACHE_DIR, { recursive: true })
  const provPath = resolve(CACHE_DIR, 'provenance.md')
  writeFileSync(
    provPath,
    `# Global DEM Enrichment Provenance

## Source
- **Copernicus GLO-30 DEM** (${DATASET.year}), ${DATASET.license}
- Bucket: s3://copernicus-dem-30m/ (public, no auth)
- Registered as dataset id ${DATASET.id} key \`${DATASET.key}\` provenance ${DATASET.provenance}

## Output
- ${DST_DIR}
- Format: 3601×3601 pixel grid, Int16 big-endian, SRTM-HGT (.hgt)
- Naming: N{lat:02d}{E|W}{lon:03d}.hgt (node-registered: pixel centres on integer degrees)
- Tile count: ${done}

## Pipeline integration
- engine/raster-reader/src/lib.rs reads dem/copernicus/ first, falls back to dem/srtm/
- Dropping newer .hgt tiles in-place is sufficient — no code changes required

## Reproduction
- \`cd pipeline && npx tsx enrich-global-dem.ts\` (convert only)
- \`cd pipeline && npx tsx enrich-global-dem.ts --download\` (also re-sync S3)
- Underlying bash: scripts/rasters/{download,convert}-dem-copernicus.sh

## Run date
${new Date().toISOString()}
`,
  )
  console.log(`  Provenance written to ${provPath}`)

  console.log('\n=== Done ===')
}

main().catch((err: unknown) => {
  console.error('FATAL:', err)
  process.exit(1)
})
