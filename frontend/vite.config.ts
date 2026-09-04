import path from 'node:path'
import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },
  build: {
    // Runtime readiness reads Vite's complete output graph instead of guessing
    // asset dependencies from index.html. That graph includes lazy chunks and
    // every CSS/asset edge emitted by Rollup.
    manifest: true,
    rollupOptions: {
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
