// The stay the places to stay are searched for, under their switch, one aligned row each: which
// places, the dates (from today for two nights), the guests, and the lowest stars and guests' score,
// every choice in view and one click away (the owner, 2026-07-29 and 2026-07-31: no drop-downs,
// stars as stars).
import { addDays, firstCheckin, withCheckin, withCheckout, type StayKind, type StaySearch } from '../lib/stays'

/** dev4's cap of the guests stepper. */
const MAX_ADULTS = 16

function Choices<T>({ label, value, options, onChange, testId }: {
  label: string
  value: T
  options: { value: T; text: string; title?: string }[]
  onChange: (value: T) => void
  testId: string
}) {
  return (
    <div role="radiogroup" aria-label={label} data-testid={testId} className="flex flex-wrap gap-1">
      {options.map(option => (
        <button
          key={option.text}
          type="button"
          role="radio"
          aria-checked={value === option.value}
          title={option.title}
          onClick={() => onChange(option.value)}
          className={`cursor-pointer rounded-md border px-1 py-px text-[11px] leading-4 tracking-tight transition-colors ${
            value === option.value
              ? 'border-foreground bg-foreground text-background'
              : 'border-border bg-background text-muted-foreground hover:bg-black/5'
          }`}
        >
          {option.text}
        </button>
      ))}
    </div>
  )
}

const DATE_INPUT = 'min-w-0 flex-1 rounded-md border border-input bg-background px-1 py-px text-[11px] leading-4 text-foreground focus:border-ring focus:outline-none'
const STEP = 'flex size-5 cursor-pointer items-center justify-center rounded-md border border-border bg-background text-foreground hover:bg-black/5 disabled:cursor-default disabled:opacity-40'

export default function StaySearchFields({ search, onChange }: { search: StaySearch; onChange: (search: StaySearch) => void }) {
  return (
    <div className="mb-1.5 ml-7 mt-0.5 grid grid-cols-[2.5rem_minmax(0,1fr)] items-center gap-x-2 gap-y-1.5 text-[11px]">
      <span className="text-muted-foreground">Type</span>
      <Choices<StayKind>
        label="Type" testId="stay-kind" value={search.kind} onChange={kind => onChange({ ...search, kind })}
        options={[
          { value: 'all', text: 'All' },
          { value: 'hotel', text: 'Hotels' },
          { value: 'rental', text: 'Apartments', title: 'Apartments, guest houses, hostels: every place that is not a hotel' },
        ]}
      />

      <span className="text-muted-foreground">Dates</span>
      <span className="flex min-w-0 items-center gap-1">
        <input
          type="date" aria-label="Check-in" data-testid="stay-checkin" className={DATE_INPUT}
          value={search.checkin} min={firstCheckin()}
          onChange={event => { if (event.target.value) onChange(withCheckin(search, event.target.value)) }}
        />
        <input
          type="date" aria-label="Check-out" data-testid="stay-checkout" className={DATE_INPUT}
          value={search.checkout} min={addDays(search.checkin, 1)}
          onChange={event => { if (event.target.value) onChange(withCheckout(search, event.target.value)) }}
        />
      </span>

      <span className="text-muted-foreground">Guests</span>
      <span className="flex items-center gap-1.5">
        <button
          type="button" className={STEP} aria-label="One guest fewer" data-testid="stay-adults-minus"
          disabled={search.adults <= 1} onClick={() => onChange({ ...search, adults: search.adults - 1 })}
        >−</button>
        <span className="w-4 text-center tabular-nums text-foreground" data-testid="stay-adults">{search.adults}</span>
        <button
          type="button" className={STEP} aria-label="One guest more" data-testid="stay-adults-plus"
          disabled={search.adults >= MAX_ADULTS} onClick={() => onChange({ ...search, adults: search.adults + 1 })}
        >+</button>
      </span>

      <span className="text-muted-foreground">Stars</span>
      <Choices
        label="Stars" testId="stay-stars" value={search.minStars} onChange={minStars => onChange({ ...search, minStars })}
        options={[
          { value: null, text: 'Any' },
          ...[3, 4, 5].map(stars => ({ value: stars, text: '★'.repeat(stars), title: `${stars} stars or more` })),
        ]}
      />

      <span className="text-muted-foreground">Score</span>
      <Choices
        label="Guests' score" testId="stay-score" value={search.minScore} onChange={minScore => onChange({ ...search, minScore })}
        options={[
          { value: null, text: 'Any' },
          ...[7, 8, 9].map(score => ({ value: score, text: `${score}+`, title: `Guests' score ${score} out of 10 or more` })),
        ]}
      />
    </div>
  )
}
