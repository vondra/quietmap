// Build artifacts for public citations; Rust owns admission and the release selects the map year.
import { resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { mkdirSync, writeFileSync } from 'node:fs'
import { execFileSync } from 'node:child_process'
import { datasetCitations } from './dataset-citations.ts'
import { readSourceManifests, yearFiles, type NamedYear, type YearManifest } from './year-sources.ts'

export function generateSourceArtifacts(): void {
  const files = yearFiles()
  const raw = readSourceManifests()
  const years: NamedYear[] = raw.map((year, index) => ({ name: year.name,
    manifest: JSON.parse(execFileSync('cargo', ['run', '--quiet', '--release', '-p', 'build', '--manifest-path', '../Cargo.toml', '--', 'manifest', '--year', files[index], '--out', '-'], { encoding: 'utf8', maxBuffer: 4 * 1024 * 1024 })) as YearManifest }))
  const built = years.filter(year => year.manifest.built)
  const selected = process.env.MAP_YEAR ? built.find(year => year.name === process.env.MAP_YEAR) : built.length === 1 ? built[0] : undefined
  if (!selected) throw new Error('MAP_YEAR must select a built year when publishing several maps')
  const directory = new URL('../.generated/', import.meta.url)
  mkdirSync(directory, { recursive: true })
  writeFileSync(new URL('years.json', directory), JSON.stringify(years))
  writeFileSync(new URL('datasets.json', directory), JSON.stringify(datasetCitations(selected.manifest)))
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) generateSourceArtifacts()
