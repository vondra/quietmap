export type PopupTab = 'sources' | 'segments'

export function TabStrip({
  active,
  sourceCount,
  segmentCount,
  onChange,
}: {
  active: PopupTab
  sourceCount: number
  segmentCount: number
  onChange: (tab: PopupTab) => void
}) {
  return (
    <div className="flex mb-0.5">
      <TabButton
        active={active === 'sources'}
        label={`Noise sources (${sourceCount})`}
        onClick={() => onChange('sources')}
      />
      <TabButton
        active={active === 'segments'}
        label={`Segments (${segmentCount})`}
        onClick={() => onChange('segments')}
      />
    </div>
  )
}

function TabButton({
  active,
  label,
  onClick,
}: {
  active: boolean
  label: string
  onClick: () => void
}) {
  const base = 'flex-1 text-[11px] uppercase tracking-[0.08em] py-1 transition-colors border-b-2'
  const variant = active
    ? 'border-foreground text-foreground font-medium'
    : 'border-transparent text-muted-foreground hover:text-foreground'
  return (
    <button className={`${base} ${variant}`} onClick={onClick}>
      {label}
    </button>
  )
}
