// Entry: the About pages under /about, else the map application with the published heatmap
// tile build.
import { StrictMode, Suspense, lazy } from 'react'
import { createRoot } from 'react-dom/client'
import './index.css'
import App from './App.tsx'
import { initTileBuild } from './lib/tile-urls'

const AboutPage = lazy(() => import('./components/AboutPage'))
// The pages the server answers (server/src/web.ts).
const about = window.location.pathname.match(/^\/about(?:\/(methodology|credits|news))?\/?$/)

// Resolve the published tile build in parallel with the React mount — the
// heatmap layers stay unmounted until the manifest lands.
if (!about) void initTileBuild()

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    {about ? <Suspense fallback={null}><AboutPage page={about[1] ?? ''} /></Suspense> : <App />}
  </StrictMode>,
)
