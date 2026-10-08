// The About pages: every markdown file under src/about, loaded when its page opens.
import { aboutPagePath } from './about-paths'

const FILES = import.meta.glob<string>('../about/**/*.md', { query: '?raw', import: 'default' })

export const ABOUT_PAGES: Readonly<Record<string, () => Promise<string>>> = Object.fromEntries(
  Object.entries(FILES).map(([file, load]) => [aboutPagePath(file.replace(/^\.\.\/about\//, '')), load]),
)
