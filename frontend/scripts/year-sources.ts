// Country and region source lists rendered from admitted year manifests, without exposing local paths.
import { globSync, readFileSync } from 'node:fs'
import { basename, join } from 'node:path'

export interface SourceInput {
  id: string
  kind: string
  region: string
  publisher: string
  url: string
  vintage: string
  licence: string
  public_ok: boolean
  description: string
}
export interface YearManifest {
  as_of: string
  note?: string
  flows: { start: string; end: string }
  inputs: SourceInput[]
  unavailable: { id: string; kind: string; region: string; reason: string }[]
}
export interface NamedYear { name: string; manifest: YearManifest }

export const YEARS_DIRECTORY = new URL('../../years', import.meta.url).pathname
export function yearFiles(): string[] {
  return globSync('*.json', { cwd: YEARS_DIRECTORY }).sort().map(file => join(YEARS_DIRECTORY, file))
}
export function readYears(): NamedYear[] {
  return yearFiles().map(file => ({ name: basename(file, '.json'), manifest: JSON.parse(readFileSync(file, 'utf8')) as YearManifest }))
}

function escapeMarkdown(text: string): string {
  return text.replace(/[\\`*_[\]<>]/g, '\\$&').replace(/\s+/g, ' ').trim()
}
function inputLine(input: SourceInput): string {
  const publisher = escapeMarkdown(input.publisher)
  const url = /^https?:\/\//.test(input.url) ? input.url.replace(/\)/g, '%29').replace(/\s/g, '%20') : ''
  const label = url ? `[${publisher}](${url})` : publisher
  return `- **${label} · ${escapeMarkdown(input.vintage)}.** ${escapeMarkdown(input.description)} Licence: ${escapeMarkdown(input.licence)}.`
}

/** The pages list local sources first. World and regional inputs remain available in a collapsed
 *  list, so a resident can find their country's counts before reading every shared model. */
export function renderYearSources(page: string, years: NamedYear[]): string {
  const parts = page.replace(/\\/g, '/').split('/')
  const region = parts[1]
  const country = basename(parts.at(-1)!, '.md').toUpperCase()
  const isCountry = country !== 'INDEX'
  const local = (scope: string) => isCountry ? scope === country || scope.startsWith(`${country}:`) : scope === region
  const shared = (scope: string) => scope === 'world' || (isCountry && scope === region)
  return [...years].sort((a, b) => b.name.localeCompare(a.name)).map(({ name, manifest }) => {
    const shown = manifest.inputs.filter(input => input.public_ok)
    const inputs = shown.filter(input => local(input.region))
    const gaps = manifest.unavailable.filter(input => local(input.region))
    const common = shown.filter(input => shared(input.region))
    const commonGaps = manifest.unavailable.filter(input => shared(input.region))
    const gapLines = (rows: typeof gaps) => [...new Set(rows.map(row => row.reason))].map(reason => `- ${escapeMarkdown(reason)}`).join('\n')
    const lines = [
      `## ${escapeMarkdown(name)}`,
      `Map state: **${manifest.as_of}**. Flow window: **${manifest.flows.start} to ${manifest.flows.end}**, ending before that last day.`,
      manifest.note ? escapeMarkdown(manifest.note) : '',
      inputs.length ? inputs.map(inputLine).join('\n\n') : 'No separate local input is admitted. The shared sources and their defaults apply.',
      gaps.length ? `**Where an earlier source is missing**\n\n${gapLines(gaps)}` : '',
      `<details>\n<summary>World${isCountry ? ' and regional' : ''} sources and models</summary>\n\n${common.map(inputLine).join('\n\n')}${commonGaps.length ? `\n\n**Where an earlier source is missing**\n\n${gapLines(commonGaps)}` : ''}\n\n</details>`,
    ]
    return lines.filter(Boolean).join('\n\n')
  }).join('\n\n')
}

export function expandYearSources(page: string, markdown: string, years: NamedYear[]): string {
  return markdown.replace('<!-- YEAR SOURCES -->', () => renderYearSources(page, years))
}
