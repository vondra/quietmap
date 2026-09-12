// Table primitives shared by the two validation QA cards (`/#val=1`). They live
// outside both so neither card has to import the other.
import type { ValidationArtifactMeta } from './ValidationLayer'

export function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <tr>
      <td className="pr-2 py-0.5 align-top whitespace-nowrap text-muted-foreground">{label}</td>
      <td className="py-0.5 tabular-nums">{children}</td>
    </tr>
  )
}

export function Link({ url, children }: { url: string | null | undefined; children?: React.ReactNode }) {
  if (!url) return null
  return <> · <a href={url} target="_blank" rel="noopener noreferrer" className="text-blue-700 underline">{children ?? 'link'}</a></>
}

/** Renders a [lo, hi] pair, either side of which may be open. */
export const range = (b: [number | null, number | null] | null | undefined) =>
  b ? (b[0] == null ? `below ${b[1]}` : b[1] == null ? `above ${b[0]}` : `${b[0]}–${b[1]}`) : '—'

export const runDay = (meta: ValidationArtifactMeta | null | undefined): string =>
  typeof meta?.generated_at === 'string' ? meta.generated_at.slice(0, 10) : ''
