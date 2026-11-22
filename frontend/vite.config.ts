// Vite build of the map: React, Tailwind, the two big vendor stacks in their own chunks, and the
// About pages: their list for the server (`about-pages.json`) and their titles for the pages'
// breadcrumbs and lists (`virtual:about-index`).
import { globSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import { defineConfig, type Plugin } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import { splitFrontMatter } from './src/lib/about-front-matter.ts'
import { aboutPagePath } from './src/lib/about-paths.ts'
import { datasetCitations } from './scripts/dataset-citations.ts'
import { generateSourceArtifacts, selectMapYear } from './scripts/generate-datasets.ts'
import { expandYearSources, readYears, yearFiles } from './scripts/year-sources.ts'

const ABOUT_DIR = new URL('./src/about', import.meta.url).pathname
const ABOUT_INDEX = 'virtual:about-index'
const VIRTUAL_MAP_YEAR = '\0generated:map-year'
const VIRTUAL_DATASETS = '\0generated:datasets'

/** The About pages (src/about): every path with its title. */
function aboutIndex(): Record<string, { title: string }> {
  return Object.fromEntries(globSync('**/*.md', { cwd: ABOUT_DIR }).sort().map(file => {
    const { meta } = splitFrontMatter(readFileSync(join(ABOUT_DIR, file), 'utf8'))
    return [aboutPagePath(file), { title: meta.title }]
  }))
}

/** The About pages' index for the frontend, and a single private per-build snapshot of the
 *  selected map year, its citations and emitted `map-year.json` so concurrent builds of
 *  different years cannot mix their outputs through `.generated/`. */
function aboutPages(): Plugin {
  let years: ReturnType<typeof readYears> = []
  let mapYear = ''
  let datasetsJson = '{}'
  const snapshot = () => {
    years = readYears()
    const selected = selectMapYear(years, process.env.MAP_YEAR)
    mapYear = selected.manifest.year
    datasetsJson = JSON.stringify(datasetCitations(selected.manifest))
  }
  return {
    buildStart() {
      snapshot()
    },
    watchChange(id) {
      if (yearFiles().includes(id)) {
        generateSourceArtifacts()
        snapshot()
      }
    },
    name: 'about-pages',
    enforce: 'pre',
    resolveId(id) {
      if (id === ABOUT_INDEX) return `\0${ABOUT_INDEX}`
      if (id.endsWith('/.generated/map-year.json')) return VIRTUAL_MAP_YEAR
      if (id.endsWith('/.generated/datasets.json')) return VIRTUAL_DATASETS
      return undefined
    },
    load(id) {
      if (id === `\0${ABOUT_INDEX}`) return `export default ${JSON.stringify(aboutIndex())}`
      if (id === VIRTUAL_MAP_YEAR) return `export default ${JSON.stringify(mapYear)}`
      if (id === VIRTUAL_DATASETS) return `export default ${datasetsJson}`
      if (!id.startsWith(`${ABOUT_DIR}/`) || !id.endsWith('.md?raw')) return
      const file = id.slice(0, -4)
      const markdown = readFileSync(file, 'utf8')
      if (!/<!-- (YEAR SOURCES|NATIONAL CLASS PRIORS|AIRCRAFT WINDOW) -->/.test(markdown)) return
      for (const year of yearFiles()) this.addWatchFile(year)
      return `export default ${JSON.stringify(expandYearSources(file.slice(ABOUT_DIR.length + 1), markdown, years))}`
    },
    generateBundle() {
      const pages = Object.keys(aboutIndex())
      this.emitFile({ type: 'asset', fileName: 'about-pages.json', source: JSON.stringify(pages) })
      this.emitFile({ type: 'asset', fileName: 'map-year.json', source: JSON.stringify(mapYear) })
    },
  }
}

export default defineConfig({
  plugins: [react(), tailwindcss(), aboutPages()],
  build: {
    // The two vendor chunks below are split out on purpose; each is one library.
    chunkSizeWarningLimit: 1100,
    rollupOptions: {
      onwarn(warning, warn) {
        // loaders.gl (under deck.gl) imports `spawn` for its Node-only worker path, never run here.
        if (warning.code === 'MISSING_EXPORT' && warning.id?.includes('@loaders.gl/worker-utils')) return
        warn(warning)
      },
      output: {
        // Split the two biggest independent vendor stacks into their own chunks
        // so an app-code change doesn't bust their cache and they download in
        // parallel. deck is still needed for first paint (all layers default
        // on), so this is a caching win, not a smaller first load.
        manualChunks(id) {
          if (!id.includes('node_modules')) return
          if (id.includes('maplibre-gl')) return 'maplibre'
          if (/deck\.gl|luma\.gl|math\.gl|wgsl_reflect|mjolnir/.test(id)) return 'deck'
        },
      },
    },
  },
})
