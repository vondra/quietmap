// Detail card for a clicked stay pin: photo, price, rating, dB, book-out link.
import { X, Star } from 'lucide-react'
import { ldenToColor } from '../utils/noise-colors'
import { formatPerNight, type Stay } from './StayLayer'

interface StayCardProps {
  stay: Stay
  onClose: () => void
}

export default function StayCard({ stay: s, onClose }: StayCardProps) {
  const noise = s.noise

  return (
    <div className="p-3">
      <div className="flex justify-between items-start mb-2">
        <h3 className="text-sm font-semibold text-foreground pr-6 leading-tight">{s.name}</h3>
        <button onClick={onClose} className="p-0.5 rounded hover:bg-muted shrink-0" aria-label="Close">
          <X className="size-3.5 text-muted-foreground" />
        </button>
      </div>

      {s.thumbnail && (
        <img src={s.thumbnail} alt={s.name} className="w-full h-32 object-cover rounded-md mb-2" />
      )}

      {s.price && (
        <div className="flex items-baseline gap-1.5 mb-1.5">
          <span className="text-lg font-bold text-foreground">{formatPerNight(s.price.perNight)}</span>
          <span className="text-xs text-muted-foreground">/ night · {formatPerNight(s.price.total)} for {s.nights} nights</span>
        </div>
      )}

      <div className="flex flex-wrap gap-1.5 mb-2 text-[11px]">
        {s.rating.value != null && (
          <span className="px-1.5 py-0.5 rounded-full bg-muted text-muted-foreground inline-flex items-center gap-0.5">
            <Star className="size-3 fill-amber-400 text-amber-400" />
            {s.rating.value.toFixed(1)}{s.rating.count != null && ` (${s.rating.count})`}
          </span>
        )}
        {s.capacity.guests != null && (
          <span className="px-1.5 py-0.5 rounded-full bg-muted text-muted-foreground">{s.capacity.guests} guests</span>
        )}
        {s.capacity.bedrooms != null && (
          <span className="px-1.5 py-0.5 rounded-full bg-muted text-muted-foreground">{s.capacity.bedrooms} bd</span>
        )}
        {s.freeCancellation && (
          <span className="px-1.5 py-0.5 rounded-full bg-emerald-100 text-emerald-700">Free cancellation</span>
        )}
      </div>

      {noise != null && (
        <div className="flex items-center gap-1.5 mb-2">
          <span
            className="inline-block w-2.5 h-2.5 rounded-full"
            style={{ backgroundColor: ldenToColor(noise) }}
          />
          <span className="text-xs text-muted-foreground">
            Noise: <strong className="text-foreground">{noise.toFixed(1)} dB</strong>
          </span>
        </div>
      )}

      <a
        href={s.url}
        target="_blank"
        rel="noopener noreferrer"
        className="block text-center text-xs font-medium text-primary-foreground bg-primary hover:opacity-90 py-1.5 rounded-md"
      >
        View deal →
      </a>
    </div>
  )
}
