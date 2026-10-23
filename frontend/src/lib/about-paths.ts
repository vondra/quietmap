// An About page's path under /about from its markdown file's path under src/about: the file's path
// without `.md`, `index` naming its directory ('' is the first page, 'europe' a region's); the
// section the popup links to.

/** How to read the popup's weather: the detailed calculation's Weather tables and a source's Share
 *  of time (a section id from `about-sections.ts`). */
export const ABOUT_WEATHER = '/about/noise-at-a-point#the-weather'

export function aboutPagePath(file: string): string {
  return file.replace(/\.md$/, '').replace(/(^|\/)index$/, '')
}

/** The page a browser path names under /about (percent-encoding kept, one trailing slash allowed),
 *  or null when the path is not under /about. */
export function aboutPageOf(pathname: string): string | null {
  const match = pathname.match(/^\/about(?:\/([^/].*?))?\/?$/)
  return match ? (match[1] ?? '') : null
}
