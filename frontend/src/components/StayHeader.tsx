// The place to stay a click on its pin opened, above the popup of its point: its photo, name, stars
// and guests' score, the price of the stay, what it offers, and the link to the offer.
import { ExternalLink } from 'lucide-react'
import { formatPrice, pricePerNight, type Stay } from '../lib/stays'

export default function StayHeader({ stay }: { stay: Stay }) {
  const night = pricePerNight(stay)
  const offers = [
    stay.guests != null ? `sleeps ${stay.guests}` : null,
    stay.bedrooms != null ? `${stay.bedrooms} bedroom${stay.bedrooms === 1 ? '' : 's'}` : null,
    stay.freeCancellation ? 'free cancellation' : null,
  ].filter(Boolean).join(' · ')
  return (
    <div data-testid="stay-header" className="flex gap-2.5 border-b border-border px-2.5 pb-2 pt-2.5">
      {stay.thumbnail && (
        <img src={stay.thumbnail} alt="" className="h-[72px] w-24 shrink-0 rounded-md bg-muted object-cover" />
      )}
      <div className="min-w-0 flex-1 pr-5">
        <div className="line-clamp-2 text-sm font-semibold leading-tight text-foreground">{stay.name}</div>
        {(stay.stars != null || stay.score != null) && (
          <div className="mt-0.5 flex items-baseline gap-1.5 text-xs text-muted-foreground">
            {stay.stars != null && stay.stars > 0 && (
              <span className="tracking-tight text-amber-500" aria-label={`${stay.stars} stars`}>{'★'.repeat(Math.round(stay.stars))}</span>
            )}
            {stay.score != null && (
              <span title="Guests' score out of 10">
                <span className="font-semibold text-foreground">{stay.score.toFixed(1)}</span>
                {stay.reviews != null && ` · ${stay.reviews.toLocaleString('en-US')} reviews`}
              </span>
            )}
          </div>
        )}
        <div className="mt-1 text-xs text-foreground" data-testid="stay-price">
          {night != null && stay.total != null
            ? <><span className="font-semibold">{formatPrice(night, stay.currency)}</span> a night · {formatPrice(stay.total, stay.currency)} for {stay.nights} night{stay.nights === 1 ? '' : 's'}</>
            : <span className="text-muted-foreground">No price for these dates</span>}
        </div>
        {offers && <div className="text-[11px] text-muted-foreground">{offers}</div>}
        <a
          href={stay.url}
          target="_blank"
          rel="noopener noreferrer sponsored"
          className="mt-1.5 inline-flex items-center gap-1 rounded-md bg-primary px-2.5 py-1 text-xs font-medium text-primary-foreground hover:opacity-90"
        >
          View offer <ExternalLink className="size-3" aria-hidden="true" />
        </a>
      </div>
    </div>
  )
}
