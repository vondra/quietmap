import type { ReactNode } from "react"

import { HoverText } from "@/components/ui/info-tip"
import { METRIC_DEFS, type MetricTerm } from "./metric-defs"

/**
 * MetricLabel — wraps a metric term from METRIC_DEFS with a native title
 * tooltip showing its definition. Use for column labels like "Screening".
 *
 * `mode` controls which copy variant is shown:
 * - `'public'` (default) — uses `descriptionPublic` if present, else
 *   falls back to `description`. Drops the `(standard)` footer.
 *   Used by the Noise Sources tab (ContributorRow) for lay-audience
 *   readability.
 * - `'technical'` — uses `description` (formulas, CNOSSOS citations,
 *   fine print). Appends `(standard)` when present. Used by the Noise
 *   Segments tab (SegmentExpanded) for pro-debug fidelity.
 */
export function MetricLabel({
  term,
  children,
  mode = "public",
}: {
  term: MetricTerm
  children?: ReactNode
  mode?: "public" | "technical"
}) {
  const def = METRIC_DEFS[term]
  if (!def) return <span>{children ?? term}</span>
  const description = mode === "technical"
    ? def.description
    : def.descriptionPublic ?? def.description
  const parts: Array<string | null | undefined> = [def.label, description]
  if (mode === "technical" && def.standard) parts.push(`(${def.standard})`)
  const titleText = parts.filter(Boolean).join("\n")
  return <HoverText title={titleText}>{children ?? def.label}</HoverText>
}

/**
 * DataPoint — wraps a value (number + unit) with a native title tooltip
 * containing the calculation explanation. Plain text only.
 */
export function DataPoint({
  text,
  children,
  title,
}: {
  /** Plain-text calculation breakdown. Use \n for line breaks. */
  text: string
  children: ReactNode
  /** Optional heading line, prepended above `text`. */
  title?: string
}) {
  const fullTitle = title ? `${title}\n\n${text}` : text
  return <HoverText title={fullTitle}>{children}</HoverText>
}
