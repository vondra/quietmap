// Provenance codes identify inputs or algorithms; publishers and observation dates come from the manifest.
import mappings from './dataset-inputs.json' with { type: 'json' }
import { observedLabel, REGIONS, type SourceInput, type YearManifest } from './year-sources.ts'

export type Dataset = readonly [name: string, observed: string, measurement: string, url: string]
const MODEL_NAMES: Record<string, string> = {
  road: 'Road-class model', rail: 'Railway-class model', continuity: 'Traffic carried along the same road',
  transition: 'Traffic and speed transition', 'timetable-residual': 'Track without scheduled service',
}
function publisherGroup(sources: SourceInput[]): string {
  const publishers = [...new Set(sources.map(source => source.publisher))]
  if (publishers.length === 1) return publishers[0]
  const shared = publishers[0].split('; ').filter(part => publishers.every(publisher => publisher.split('; ').includes(part)))
  if (shared.length) return shared.join('; ')
  const kinds = new Set(sources.map(source => source.kind))
  if (kinds.size === 1 && kinds.has('prepare-timetable')) return 'Public timetables'
  return publishers.join('; ')
}
export function datasetCitations(manifest: YearManifest): Record<number, Dataset> {
  const inputs = new Map(manifest.inputs.map(input => [input.id, input]))
  return Object.fromEntries(Object.entries(mappings).flatMap(([code, mapping]) => {
    if ('model' in mapping) return [[code, [MODEL_NAMES[mapping.model], '', 'proxy', '']]]
    const sources = mapping.inputs.flatMap(id => inputs.has(id) ? [inputs.get(id)!] : [])
    if (!sources.length) return []
    const observations = [...new Set(sources.map(input => observedLabel(input.observed)))]
    // A shared output code cannot identify one city's survey: show its published range once.
    const years = sources.flatMap(input => [input.observed.start.slice(0, 4), input.observed.end.slice(0, 4)]).sort()
    const observed = observations.length === 1 ? observations[0] : observedLabel({ start: years[0], end: years.at(-1)! })
    const urls = new Set(sources.map(input => input.url))
    const regions = new Set(sources.map(input => input.region.split(':')[0]))
    const url = urls.size === 1 ? sources[0].url : `/about/${REGIONS[regions.size === 1 ? [...regions][0] : 'world']}`
    return [[code, [publisherGroup(sources), observed, mapping.measurement, url]]]
  }))
}
