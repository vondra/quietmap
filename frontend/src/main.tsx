import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import './index.css'
import App from './App.tsx'
import { initTileBuild } from './lib/tile-urls'

// Resolve the published tile build in parallel with the React mount — the
// heatmap layers stay unmounted until the manifest lands.
void initTileBuild()

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
