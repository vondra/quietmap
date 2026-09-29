// Tooltip wrappers of the popup: a metric's definition on its label, a calculation on its value.
import type { ReactNode } from "react"

import { HoverText } from "../ui/info-tip"
import { METRIC_DEFS, type MetricTerm } from "./metric-defs"

/** A metric term from METRIC_DEFS with its plain-language definition as the tooltip. */
export function MetricLabel({
  term,
  children,
}: {
  term: MetricTerm
  children?: ReactNode
}) {
  const def = METRIC_DEFS[term]
  return <HoverText title={`${def.label}\n${def.description}`}>{children ?? def.label}</HoverText>
}

/**
 * DataPoint — wraps a value (number + unit) with a tooltip containing the
 * calculation explanation. Plain text only.
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
