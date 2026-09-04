/** Single writer for the document (tab/share) title.
 *  Patterns (owner spec 2026-07-10): default "quietmap.org - Find your quiet place";
 *  with a popup open, PLACE comes first, never the number:
 *  "Dejvice, Praha - 62 dB - quietmap.org".
 *  Kept in sync manually with frontend/index.html <title>/og:title — static
 *  HTML can't import this module, and crawlers read it without executing JS. */
const BRAND = 'quietmap.org'
const CLAIM = 'Find your quiet place'
const DEFAULT_DOCUMENT_TITLE = `${BRAND} - ${CLAIM}`

export function setDocumentTitle(parts: Array<string | null>): void {
  const body = parts.filter((p): p is string => Boolean(p))
  document.title = body.length ? `${body.join(' - ')} - ${BRAND}` : DEFAULT_DOCUMENT_TITLE
}
