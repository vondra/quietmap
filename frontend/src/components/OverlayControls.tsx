import { useState, useEffect, useRef } from 'react'
import { TreePine, BedDouble } from 'lucide-react'
import type { RealEstateFilters } from './RealEstateLayer'
import type { StayFilters } from './StayLayer'
import { QUIET_THRESHOLD_MIN, QUIET_THRESHOLD_MAX, QUIET_THRESHOLD_STEP } from '../hooks/useUrlState'
import { Switch } from './ui/switch'
import { CheckChip } from './ui/check-chip'

interface OverlayControlsProps {
  quietClustersEnabled: boolean
  onQuietClustersChange: (enabled: boolean) => void
  quietThreshold: number
  onQuietThresholdChange: (threshold: number) => void
  realEstateFilters: RealEstateFilters
  onRealEstateChange: (filters: RealEstateFilters) => void
  stayFilters: StayFilters
  onStayChange: (filters: StayFilters) => void
}

function ToggleRow({ active, icon, label, tooltip, onClick }: {
  active: boolean; icon: React.ReactNode; label: string; tooltip: string; onClick: () => void
}) {
  return (
    <button
      onClick={onClick}
      title={tooltip}
      className="flex w-full items-center gap-2.5 py-1.5 px-1 rounded-lg hover:bg-black/5 transition-colors cursor-pointer"
    >
      <span className={active ? 'text-foreground' : 'text-muted-foreground'}>{icon}</span>
      <span className={`flex-1 text-left text-sm ${active ? 'text-foreground' : 'text-muted-foreground'}`}>{label}</span>
      <Switch on={active} />
    </button>
  )
}

function NoiseSlider({ value, onChange, min, max, step = 1, testId }: {
  value: number; onChange: (v: number) => void; min: number; max: number; step?: number; testId: string
}) {
  const [local, setLocal] = useState(value)
  const onChangeRef = useRef(onChange)
  onChangeRef.current = onChange
  useEffect(() => { setLocal(value) }, [value])
  useEffect(() => {
    if (local === value) return
    const t = setTimeout(() => onChangeRef.current(local), 300)
    return () => clearTimeout(t)
  }, [local, value])
  return (
    <div className="flex items-center gap-2 ml-7 mt-0.5 mb-1">
      <span className="text-[11px] text-muted-foreground shrink-0">below</span>
      <input type="range" data-testid={testId} value={local}
        onChange={(e) => setLocal(parseFloat(e.target.value))} min={min} max={max} step={step}
        className="flex-1 h-1 accent-primary cursor-pointer" />
      <span className="text-[11px] text-muted-foreground tabular-nums w-12 text-right">{local} dB</span>
    </div>
  )
}

const inputCls = 'rounded-md border border-input bg-background px-1.5 py-0.5 text-[11px] text-foreground focus:border-ring focus:outline-none'

/** Debounced numeric field — typing must not fire a live re-fetch per
 *  keystroke, but a pending edit must never be LOST: blur and unmount both
 *  flush it (collapsing the panel mid-edit used to discard the value). */
function NumberField({ value, onCommit, min, max, placeholder, width, testId }: {
  value: number | null; onCommit: (v: number | null) => void
  min: number; max: number; placeholder: string; width: string; testId: string
}) {
  const [local, setLocal] = useState(value == null ? '' : String(value))
  const flushRef = useRef(() => {})
  useEffect(() => { setLocal(value == null ? '' : String(value)) }, [value])
  const n = Number(local)
  const parsed = local === '' || !Number.isFinite(n) ? null : Math.min(max, Math.max(min, Math.trunc(n)))
  flushRef.current = () => { if (parsed !== value) onCommit(parsed) }
  useEffect(() => {
    const t = setTimeout(() => flushRef.current(), 500)
    return () => clearTimeout(t)
  }, [local])
  useEffect(() => () => flushRef.current(), [])
  return (
    <input type="number" inputMode="numeric" data-testid={testId} value={local} min={min} max={max}
      placeholder={placeholder} onChange={(e) => setLocal(e.target.value)} onBlur={() => flushRef.current()}
      className={`${inputCls} ${width}`} />
  )
}

/** Local calendar date (not UTC — a user behind UTC could otherwise not pick
 *  their own today) + day arithmetic in local time. */
function localDate(offsetDays = 0): string {
  const d = new Date()
  d.setDate(d.getDate() + offsetDays)
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`
}

const DAY_MS = 86_400_000

/** Radio-style pill group — ≤5 options stay VISIBLE (one click), never a
 *  dropdown (two clicks + hidden values); single-select counterpart of
 *  CheckChip per the same style note. */
function PillGroup({ options, value, onChange, testId }: {
  options: { v: number | null; label: string }[]
  value: number | null
  onChange: (v: number | null) => void
  testId: string
}) {
  return (
    <div className="flex gap-1" data-testid={testId}>
      {options.map(({ v, label }) => (
        <button
          key={label}
          type="button"
          onClick={() => onChange(v)}
          aria-pressed={value === v}
          className={`rounded-md border px-1 py-0.5 text-[11px] tracking-tight cursor-pointer transition-colors ${
            value === v
              ? 'border-foreground bg-foreground text-background'
              : 'border-border bg-background text-muted-foreground hover:bg-black/5'
          }`}
        >
          {label}
        </button>
      ))}
    </div>
  )
}

/**
 * Stay filters, patterned on Google Travel / Booking (owner 2026-07-29):
 * dates always prefilled (today + 2 nights — an empty date field reads as
 * broken), guests as a stepper, price labelled as a cap, stars and guest
 * rating as one-click visible pills. Label column keeps the rows aligned.
 */
function StayFilterBlock({ filters, onChange }: { filters: StayFilters; onChange: (f: StayFilters) => void }) {
  const today = localDate()
  const adults = filters.adults ?? 2
  // Moving check-in keeps the stay LENGTH (Booking/Google behaviour) — the
  // server would otherwise silently fall back to defaults on an inverted
  // range while the UI kept displaying it.
  const changeCheckin = (checkin: string | null) => {
    if (!checkin) { onChange({ ...filters, checkin: null }); return }
    const nights = filters.checkin && filters.checkout
      ? Math.max(1, Math.min(30, Math.round((Date.parse(filters.checkout) - Date.parse(filters.checkin)) / DAY_MS)))
      : 2
    const checkout = new Date(Date.parse(checkin) + nights * DAY_MS).toISOString().slice(0, 10)
    onChange({ ...filters, checkin, checkout })
  }
  const stepBtn = 'flex size-5 items-center justify-center rounded-md border border-border bg-background text-foreground cursor-pointer hover:bg-black/5 disabled:opacity-40 disabled:cursor-default'
  return (
    <div className="ml-7 mt-1 mb-1.5 grid grid-cols-[34px_1fr] items-center gap-x-2 gap-y-1.5 text-[11px]">
      <span className="col-span-2 flex gap-1.5">
        <CheckChip
          checked={filters.hotels}
          label="Hotels"
          onToggle={() => onChange({ ...filters, hotels: !filters.hotels })}
          testId="stay-hotels"
        />
        <CheckChip
          checked={filters.rentals}
          label="Apartments"
          onToggle={() => onChange({ ...filters, rentals: !filters.rentals })}
          testId="stay-rentals"
        />
      </span>

      <span className="text-muted-foreground">Dates</span>
      <span className="flex items-center gap-1">
        <input type="date" data-testid="stay-checkin" value={filters.checkin ?? ''}
          min={today} max={localDate(540)}
          onChange={(e) => changeCheckin(e.target.value || null)} className={`${inputCls} min-w-0 flex-1`} />
        <input type="date" data-testid="stay-checkout" value={filters.checkout ?? ''}
          min={filters.checkin ? new Date(Date.parse(filters.checkin) + DAY_MS).toISOString().slice(0, 10) : today}
          max={filters.checkin ? new Date(Date.parse(filters.checkin) + 30 * DAY_MS).toISOString().slice(0, 10) : undefined}
          onChange={(e) => onChange({ ...filters, checkout: e.target.value || null })} className={`${inputCls} min-w-0 flex-1`} />
      </span>

      <span className="text-muted-foreground">Guests</span>
      <span className="flex items-center gap-1.5">
        <button type="button" className={stepBtn} data-testid="stay-adults-minus" disabled={adults <= 1}
          onClick={() => onChange({ ...filters, adults: Math.max(1, adults - 1) })}>−</button>
        <span className="w-4 text-center text-foreground tabular-nums" data-testid="stay-adults">{adults}</span>
        <button type="button" className={stepBtn} data-testid="stay-adults-plus" disabled={adults >= 16}
          onClick={() => onChange({ ...filters, adults: Math.min(16, adults + 1) })}>+</button>
      </span>

      <span className="text-muted-foreground">Price</span>
      <span className="flex items-center gap-1 text-muted-foreground">
        <span>up to</span>
        <NumberField value={filters.maxPrice} onCommit={(v) => onChange({ ...filters, maxPrice: v })}
          min={1} max={99999} placeholder="any" width="w-14" testId="stay-maxprice" />
        <span>€/night</span>
      </span>

      <span className="text-muted-foreground">Stars</span>
      <PillGroup testId="stay-stars" value={filters.minStars}
        options={[{ v: null, label: 'Any' }, { v: 3, label: '★★★' }, { v: 4, label: '★★★★' }, { v: 5, label: '★★★★★' }]}
        onChange={(v) => onChange({ ...filters, minStars: v })} />

      <span className="text-muted-foreground">Rating</span>
      <PillGroup testId="stay-rating" value={filters.minRating}
        options={[{ v: null, label: 'Any' }, { v: 7, label: '7+' }, { v: 8, label: '8+' }, { v: 9, label: '9+' }]}
        onChange={(v) => onChange({ ...filters, minRating: v })} />
    </div>
  )
}

export default function OverlayControls({
  quietClustersEnabled, onQuietClustersChange,
  quietThreshold, onQuietThresholdChange,
  stayFilters, onStayChange,
}: OverlayControlsProps) {
  return (
    <div>
      <ToggleRow
        active={quietClustersEnabled}
        icon={<TreePine className="size-4" />}
        label="Quiet zones"
        tooltip="Highlight areas where total noise (all sources) stays below a threshold"
        onClick={() => onQuietClustersChange(!quietClustersEnabled)}
      />
      {quietClustersEnabled && (
        <NoiseSlider value={quietThreshold} onChange={onQuietThresholdChange} min={QUIET_THRESHOLD_MIN} max={QUIET_THRESHOLD_MAX} step={QUIET_THRESHOLD_STEP} testId="quiet-threshold" />
      )}

      <ToggleRow
        active={stayFilters.enabled}
        icon={<BedDouble className="size-4" />}
        label="Places to stay"
        tooltip="Bookable hotels and apartments with live prices and noise levels"
        onClick={() => onStayChange({ ...stayFilters, enabled: !stayFilters.enabled })}
      />
      {stayFilters.enabled && <StayFilterBlock filters={stayFilters} onChange={onStayChange} />}

      {/* Properties (real estate) HIDDEN before launch (owner 2026-07-15): the data
          pipeline isn't ready and a dead toggle would confuse visitors. The layer,
          filters, URL state and props all stay wired — restore by re-adding the
          ToggleRow + filter block (git history of this file has the exact JSX). */}
    </div>
  )
}
