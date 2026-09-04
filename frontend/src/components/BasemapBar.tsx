import { BASEMAPS, type BasemapId } from '../utils/basemaps'

const ICONS: Record<BasemapId, React.ReactNode> = {
  standard: (
    <svg className="size-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round">
      <rect x="3" y="3" width="18" height="18" rx="2" />
      <path d="M3 9h18M3 15h18M9 3v18M15 3v18" />
    </svg>
  ),
  terrain: (
    <svg className="size-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round">
      <path d="M2 19l5-10 4 6 3-8 8 12H2z" />
    </svg>
  ),
  satellite: (
    <svg className="size-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round">
      <circle cx="12" cy="12" r="9" />
      <path d="M12 3c-2.5 3-4 6.5-4 9s1.5 6 4 9M12 3c2.5 3 4 6.5 4 9s-1.5 6-4 9M3 12h18" />
    </svg>
  ),
}

// 44 px tap target on mobile (Apple HIG min), 29 px on desktop where a mouse
// is precise — the mobile row and the desktop column share this class.
const BTN = 'flex items-center justify-center w-11 h-11 md:w-[29px] md:h-[29px] cursor-pointer'

interface BasemapBarProps {
  basemap: BasemapId
  onBasemapChange: (id: BasemapId) => void
  /** Mobile-only locate box in this row — fires the map's GeolocateControl.
   *  Living IN the row guarantees one baseline and identical margins/sizes
   *  (owner report 2026-07-16: the separately-positioned maplibre button
   *  drifted below the row and clipped under Safari's toolbar). */
  onLocate?: () => void
  /** True while the map follows the user's position (blue icon). */
  locating?: boolean
  /** False until the map's GeolocateControl is mounted — a tap before that
   *  would silently do nothing, so the box renders disabled. */
  locateReady?: boolean
}

export default function BasemapBar({ basemap, onBasemapChange, onLocate, locating, locateReady }: BasemapBarProps) {
  return (
    <>
      {/* Desktop: vertical column, bottom-left — a gap above the locate
          button, which sits a gap above +/- (Google-like stack). */}
      <div className="hidden md:flex flex-col items-start fixed bottom-[131px] left-[10px] z-[1002]">
        <div className="rounded bg-white overflow-hidden" style={{ boxShadow: '0 0 0 2px rgba(0,0,0,.1)' }}>
          {BASEMAPS.map((b, i) => (
            <button
              key={b.id}
              onClick={() => onBasemapChange(b.id)}
              title={b.label}
              className={`${BTN} transition-colors ${
                basemap === b.id
                  ? 'bg-primary text-primary-foreground'
                  : 'text-[#333] hover:bg-[#f2f2f2]'
              } ${i > 0 ? 'border-t border-[rgba(0,0,0,0.1)]' : ''}`}
            >
              {ICONS[b.id]}
            </button>
          ))}
        </div>
      </div>

      {/* Mobile: horizontal row, bottom-left — basemap trio + locate box share
          ONE flex container, so their baseline, margins and sizes can never
          drift apart. */}
      <div className="flex md:hidden items-center gap-[10px] fixed bottom-[16px] left-[10px] z-[1002]">
        <div className="flex rounded-lg bg-white overflow-hidden" style={{ boxShadow: '0 0 0 2px rgba(0,0,0,.1)' }}>
          {BASEMAPS.map((b, i) => (
            <button
              key={b.id}
              onClick={() => onBasemapChange(b.id)}
              title={b.label}
              className={`${BTN} transition-colors ${
                basemap === b.id
                  ? 'bg-primary text-primary-foreground'
                  : 'text-[#333] hover:bg-[#f2f2f2]'
              } ${i > 0 ? 'border-l border-[rgba(0,0,0,0.1)]' : ''}`}
            >
              {ICONS[b.id]}
            </button>
          ))}
        </div>
        {onLocate && (
          <button
            onClick={onLocate}
            disabled={!locateReady}
            aria-pressed={!!locating}
            title={locating ? 'Stop following my location' : 'Show my location'}
            aria-label={locating ? 'Stop following my location' : 'Show my location'}
            className={`${BTN} rounded-lg bg-white transition-colors disabled:opacity-40 disabled:cursor-default ${
              locating ? 'text-[#1565c0]' : 'text-[#333] hover:bg-[#f2f2f2]'
            }`}
            style={{ boxShadow: '0 0 0 2px rgba(0,0,0,.1)' }}
          >
            <svg className="size-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round">
              <line x1="2" x2="5" y1="12" y2="12" />
              <line x1="19" x2="22" y1="12" y2="12" />
              <line x1="12" x2="12" y1="2" y2="5" />
              <line x1="12" x2="12" y1="19" y2="22" />
              <circle cx="12" cy="12" r="7" />
              {locating && <circle cx="12" cy="12" r="2.5" fill="currentColor" stroke="none" />}
            </svg>
          </button>
        )}
      </div>
    </>
  )
}
