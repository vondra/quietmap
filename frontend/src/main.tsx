// Entry: the About pages under /about, else the map application with the published heatmap
// tile build.
import { StrictMode, Suspense, lazy } from 'react'
import { createRoot } from 'react-dom/client'
import './index.css'
import App from './App.tsx'
import { initTileBuild } from './lib/tile-urls'
import { aboutPageOf } from './lib/about-paths'

const AboutPage = lazy(() => import('./components/AboutPage'))
// The server answers only the pages the build lists (server/src/web.ts).
const about = aboutPageOf(window.location.pathname)

// Resolve the published tile build in parallel with the React mount — the
// heatmap layers stay unmounted until the manifest lands.
if (about === null) void initTileBuild()

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    {about !== null ? <Suspense fallback={null}><AboutPage page={about} /></Suspense> : <App />}
  </StrictMode>,
)
