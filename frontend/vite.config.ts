// Vite build of the map: React, Tailwind, and the two big vendor stacks in their own chunks.
import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

export default defineConfig({
  plugins: [react(), tailwindcss()],
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
