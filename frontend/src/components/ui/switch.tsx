/**
 * Pill toggle switch (display-only — the parent <button> owns the click).
 * Two sizes match the existing layer rows: `default` (h-5 w-9) for the main
 * layer/quiet toggles, `sm` (h-4 w-7) for the denser Advanced raster rows.
 */
const SIZES = {
  default: { track: 'h-5 w-9', knob: 'size-3.5', on: 'translate-x-[18px]', off: 'translate-x-[3px]' },
  sm: { track: 'h-4 w-7', knob: 'size-3', on: 'translate-x-[14px]', off: 'translate-x-[2px]' },
} as const

export function Switch({ on, size = 'default' }: { on: boolean; size?: keyof typeof SIZES }) {
  const s = SIZES[size]
  return (
    <span
      className={`relative inline-flex ${s.track} shrink-0 items-center rounded-full transition-colors ${
        on ? 'bg-primary' : 'bg-muted-foreground/20'
      }`}
    >
      <span
        className={`inline-block ${s.knob} rounded-full bg-white shadow-sm transition-transform ${
          on ? s.on : s.off
        }`}
      />
    </span>
  )
}
