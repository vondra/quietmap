// Build artifacts for public citations; Rust owns admission and the release selects the map year.
import { resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { mkdirSync, writeFileSync } from 'node:fs'
import { execFileSync } from 'node:child_process'
import { datasetCitations } from './dataset-citations.ts'
import { readSourceManifests, yearFiles, type NamedYear, type YearManifest } from './year-sources.ts'

const YEAR_ID = /^\d{4}$/

/** Select the single built map year to publish and verify its filename matches its manifest year. */
export function selectMapYear(years: readonly NamedYear[], envYear?: string): NamedYear {
  const built = years.filter(year => year.manifest.built)
  const selected = envYear
    ? built.find(year => year.name === envYear)
    : built.length === 1 ? built[0] : undefined
  if (!selected) throw new Error('MAP_YEAR must select a built year when publishing several maps')
  if (!YEAR_ID.test(selected.manifest.year) || selected.name !== selected.manifest.year) {
    throw new Error(`Year manifest filename ${selected.name} disagrees with manifest year ${selected.manifest.year}`)
  }
  return selected
}

export function generateSourceArtifacts(): void {
  const files = yearFiles()
  const raw = readSourceManifests()
  const years: NamedYear[] = raw.map((year, index) => ({ name: year.name,
    manifest: JSON.parse(execFileSync('cargo', ['run', '--quiet', '--release', '-p', 'build', '--manifest-path', '../Cargo.toml', '--', 'manifest', '--year', files[index], '--out', '-'], { encoding: 'utf8', maxBuffer: 4 * 1024 * 1024 })) as YearManifest }))
  const selected = selectMapYear(years, process.env.MAP_YEAR)
  const directory = new URL('../.generated/', import.meta.url)
  mkdirSync(directory, { recursive: true })
  writeFileSync(new URL('years.json', directory), JSON.stringify(years))
  writeFileSync(new URL('datasets.json', directory), JSON.stringify(datasetCitations(selected.manifest)))
  writeFileSync(new URL('map-year.json', directory), JSON.stringify(selected.manifest.year))
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) generateSourceArtifacts()
