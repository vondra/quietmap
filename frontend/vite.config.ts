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

const ABOUT_DIR = new URL('./src/about', import.meta.url).pathname
const ABOUT_INDEX = 'virtual:about-index'

/** The About pages (src/about): every path with its title, and whether its parent lists it. */
function aboutIndex(): Record<string, { title: string; hidden?: true }> {
  return Object.fromEntries(globSync('**/*.md', { cwd: ABOUT_DIR }).sort().map(file => {
    const { meta } = splitFrontMatter(readFileSync(join(ABOUT_DIR, file), 'utf8'))
    return [aboutPagePath(file), meta.nav === 'hidden' ? { title: meta.title, hidden: true } : { title: meta.title }]
  }))
}

/** The About pages' index for the frontend, and their paths for the server: any other /about path
 *  is a 404. */
function aboutPages(): Plugin {
  return {
    name: 'about-pages',
    resolveId: id => (id === ABOUT_INDEX ? `\0${ABOUT_INDEX}` : undefined),
    load: id => (id === `\0${ABOUT_INDEX}` ? `export default ${JSON.stringify(aboutIndex())}` : undefined),
    generateBundle() {
      const pages = Object.keys(aboutIndex())
      this.emitFile({ type: 'asset', fileName: 'about-pages.json', source: JSON.stringify(pages) })
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
