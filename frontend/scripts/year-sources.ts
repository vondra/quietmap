// Public source sections and popup citations use the release's year manifests.
import { globSync, readFileSync } from 'node:fs'
import { basename, join } from 'node:path'
import { INPUT_KINDS, type InputKind } from './input-kinds.ts'

export interface Interval { start: string; end: string }
export interface SourceInput {
  id: string
  kind: InputKind
  region: string
  publisher: string
  url: string
  observed: Interval
  published: string | null
  licence: string
  public_ok: boolean
  depends_on: string[]
  description: string
  conversion?: Record<string, unknown>
}
export interface YearManifest {
  year: string
  as_of: string
  built: boolean
  flows: { start: string; end: string; excluded_days?: string[]; increment_days?: string[] }
  inputs: SourceInput[]
  railways?: { objects: { input: string; source_id?: number }[] }
  unavailable: { id: string; kind: InputKind; region: string; description: string; depends_on: string[] }[]
  validity?: Record<string, Interval>
  shared?: string[]
}
export interface NamedYear { name: string; manifest: YearManifest }

export const YEARS_DIRECTORY = new URL('../../years', import.meta.url).pathname
export const REGIONS: Record<string, string> = JSON.parse(readFileSync(join(YEARS_DIRECTORY, 'regions.json'), 'utf8'))
export const PRIVATE_SOURCE_TEXT = /\b(?:enrichment|paged)\b|\b[\w-]+[-_]cache\b|\bw\d+-\w+|\b[\w-]+[-_]window\b|(?:^|\s)\/(?!\/)[^\s]+|\bdev\d+\b|\b(?:source|dataset)[ _-]?id\s*[=:]|keeps its behaviour|\[latest\]|\{[^}]*\.\.[^}]*\}/i

export function yearFiles(): string[] {
  return globSync('[0-9][0-9][0-9][0-9].json', { cwd: YEARS_DIRECTORY }).sort().map(file => join(YEARS_DIRECTORY, file))
}
export function validatePublicManifest(manifest: YearManifest): void {
  if (typeof manifest.built !== 'boolean') throw new Error('A release must declare whether its map is built')
  for (const input of [...manifest.inputs, ...manifest.unavailable]) {
    if (!(INPUT_KINDS as readonly string[]).includes(input.kind)) throw new Error(`Unknown input kind ${input.kind}`)
    if (!REGIONS[input.region.split(':')[0]]) throw new Error(`Unknown About region ${input.region}`)
    if (Object.values(input).some(value => typeof value === 'string' && PRIVATE_SOURCE_TEXT.test(value))) throw new Error(`Private or unpinned source text in ${input.id}`)
  }
  const fields = ['id', 'kind', 'region', 'publisher', 'url', 'observed', 'published', 'licence', 'public_ok', 'depends_on', 'description', 'conversion']
  for (const input of manifest.inputs) {
    if (Object.keys(input).some(key => !fields.includes(key))) throw new Error(`Non-public field in ${input.id}`)
    if (!input.observed?.start || !input.observed.end) throw new Error(`Missing observation interval in ${input.id}`)
    if (!/^https?:\/\/[^\s]+$/.test(input.url)) throw new Error(`Invalid public URL in ${input.id}`)
    if (/latest/i.test(input.url)) throw new Error(`Moving source URL in ${input.id}`)
  }
}
export function readSourceManifests(): NamedYear[] {
  return yearFiles().map(file => {
    const manifest = JSON.parse(readFileSync(file, 'utf8')) as YearManifest
    validatePublicManifest(manifest)
    return { name: basename(file, '.json'), manifest }
  })
}
/** Admission is computed by the Rust validator during prebuild, not reimplemented here. */
export function readYears(): NamedYear[] {
  const years = JSON.parse(readFileSync(new URL('../.generated/years.json', import.meta.url), 'utf8')) as NamedYear[]
  for (const year of years) validatePublicManifest(year.manifest)
  return years
}
function escapeMarkdown(text: string): string {
  return text.replace(/[\\`*_[\]<>]/g, '\\$&').replace(/\s+/g, ' ').trim()
}
export function observedLabel(interval: Interval): string {
  return interval.start === interval.end ? interval.end : `${interval.start} to ${interval.end}`
}
function inputLine(input: SourceInput): string {
  const publisher = escapeMarkdown(input.publisher)
  const url = /^https?:\/\//.test(input.url) ? input.url.replace(/\)/g, '%29').replace(/\s/g, '%20') : ''
  const label = url ? `[${publisher}](${url})` : publisher
  const publication = input.published ? ` Published ${escapeMarkdown(input.published)}.` : ' Publication date not recorded.'
  return `- ${label} · **${escapeMarkdown(observedLabel(input.observed))}**.${publication} ${escapeMarkdown(input.description)} Licence: ${escapeMarkdown(input.licence)}.`
}

/** Only the list is generated. Method text stays on its country or region page, shared
 * inputs are listed once where their scope belongs, and unfinished maps remain unpublished. */
export function renderYearSources(page: string, years: NamedYear[]): string {
  const path = page.replace(/\\/g, '/').replace(/\/index\.md$/, '').replace(/\.md$/, '')
  const scope = Object.entries(REGIONS).find(([, target]) => target === path)?.[0]
  if (!scope) throw new Error(`No source region for ${page}`)
  const local = (region: string) => region === scope || region.startsWith(`${scope}:`)
  const sharedLinks = scope === 'world' ? '' : [
    `See the [world sources](/about/data-sources#sources-by-year).`,
    scope.length === 2 && path.split('/').length > 2 ? `The [regional sources](/about/${path.split('/').slice(0, -1).join('/')}#sources-by-year) apply too.` : '',
  ].filter(Boolean).join(' ')
  const sections = [...years].filter(year => year.manifest.built).sort((a, b) => b.name.localeCompare(a.name)).map(({ name, manifest }) => {
    const intermediate = new Set<InputKind>(['prepared', 'prepare-regions', 'prepare-osm-boundaries', 'osm-geography', 'structure-observations', 'structures', 'road-conversion', 'road-observations', 'road-calibration', 'road-preparation-model', 'rasters', 'airport-traffic', 'road-traffic', 'segments', 'shuffled', 'terrain', 'weather', 'bus', 'worship', 'venues', 'water', 'turbines', 'barriers', 'geoid', 'raster-mosaic', 'ships', 'prepare-weather-statistics', 'facilities', 'facility-observations'])
    const conversion = manifest.inputs.find(input => input.kind === 'road-conversion')
    const selectedRoadCount = (input: SourceInput) => input.kind === 'road-reference-counts' || input.kind === 'prepare-road-counts' && (!conversion || conversion.depends_on.includes(input.id))
    const inputs = manifest.inputs.filter(input => local(input.region) && !intermediate.has(input.kind)
      && (input.kind !== 'prepare-road-counts' || selectedRoadCount(input))
      && (input.kind !== 'prepare-road-network' || !conversion || conversion.depends_on.includes(input.id)))
    const noRoadCounts = scope.length === 2 && !inputs.some(selectedRoadCount)
    // public_ok governs redistribution of records, not attribution of their publisher.
    const lines = inputs.map(inputLine)
    return [
      `### ${escapeMarkdown(name)}`,
      `Map state: immediately before **${manifest.as_of}**.${scope === 'world' ? ` Flows: **${manifest.flows.start} to ${manifest.flows.end}**, excluding the last day.${flowExclusions(manifest)}` : ''}`,
      noRoadCounts ? 'No road count at or before this year is used for this country. Traffic follows the [global model](/about/data-preparation) and the demand from mapped buildings.' : '',
      lines.length ? lines.join('\n\n') : 'No separate local dataset; the world and regional sources apply.',
    ].filter(Boolean).join('\n\n')
  })
  return ['## Sources by year', sharedLinks, ...sections].filter(Boolean).join('\n\n')
}
export function expandYearSources(page: string, markdown: string, years: NamedYear[]): string {
  return markdown.replace('<!-- YEAR SOURCES -->', () => renderYearSources(page, years))
    .replace('<!-- NATIONAL CLASS PRIORS -->', () => renderClassPriors(years))
    .replace('<!-- AIRCRAFT WINDOW -->', () => renderAircraftWindows(years))
}

/** Numeric method tables are published from the same values the converter reads. */
export function renderClassPriors(years: NamedYear[]): string {
  const countries = new Intl.DisplayNames(['en'], { type: 'region' })
  return years.filter(year => year.manifest.built).map(({ name }) => {
    const table = JSON.parse(readFileSync(join(YEARS_DIRECTORY, 'tables', name, 'road-class-priors.json'), 'utf8')) as { rows: { country?: string; countries?: string[]; value: number[][] }[] }
    const rows = table.rows.flatMap(row => row.value.map((values, index) =>
      `| ${row.country ? countries.of(row.country) : row.countries?.length ? row.countries.map(country => countries.of(country)).join(', ') : 'World'} | ${index === 0 ? 'Secondary' : 'Tertiary'} | ${values.map(value => value.toLocaleString('en')).join(' | ')} |`))
    return [`The ${name} map’s national class medians, in vehicles per day:`,
      '| Country | Road | Unknown surroundings | Rural | Urban |', '|---|---|---:|---:|---:|', ...rows].join('\n')
  }).join('\n\n')
}
export function renderAircraftWindows(years: NamedYear[]): string {
  return years.filter(year => year.manifest.built).map(({ name, manifest }) =>
    `The ${name} map uses [adsb.lol](https://adsb.lol/) flights from ${manifest.flows.start} to ${manifest.flows.end} (excluding the last day).${flowExclusions(manifest)} Flights received only by [ADSBExchange](https://www.adsbexchange.com/) are added from ${manifest.flows.increment_days?.length ?? 0} sample days.`).join('\n\n')
}

function flowExclusions(manifest: YearManifest): string {
  const days = manifest.flows.excluded_days ?? []
  return days.length ? ` Also excluded (${days.length} missing ${days.length === 1 ? 'day' : 'days'}): ${days.join(', ')}.` : ''
}
