import { X } from 'lucide-react'
import { ldenToColor } from '../utils/noise-colors'
import type { Property } from './RealEstateLayer'

interface PropertyCardProps {
  property: Property
  onClose: () => void
}

function formatPrice(price: number, currency: string, listing: string): string {
  const formatted = price >= 1_000_000
    ? `${(price / 1_000_000).toFixed(1)} M ${currency}`
    : `${price.toLocaleString()} ${currency}`
  return listing === 'rent' ? `${formatted}/month` : formatted
}

export default function PropertyCard({ property: p, onClose }: PropertyCardProps) {
  return (
    <div className="p-3">
      <div className="flex justify-between items-start mb-2">
        <h3 className="text-sm font-semibold text-foreground pr-6 leading-tight">{p.title}</h3>
        <button onClick={onClose} className="p-0.5 rounded hover:bg-muted shrink-0" aria-label="Close">
          <X className="size-3.5 text-muted-foreground" />
        </button>
      </div>

      {p.photo && (
        <img src={p.photo} alt={p.title} className="w-full h-32 object-cover rounded-md mb-2" />
      )}

      <div className="flex items-center gap-2 mb-1.5">
        <span className="text-lg font-bold text-foreground">{formatPrice(p.price, p.currency, p.listing)}</span>
      </div>

      <div className="flex flex-wrap gap-1.5 mb-2 text-[11px]">
        <span className={`px-1.5 py-0.5 rounded-full ${p.listing === 'buy' ? 'bg-blue-100 text-blue-700' : 'bg-amber-100 text-amber-700'}`}>
          {p.listing === 'buy' ? 'For sale' : 'For rent'}
        </span>
        <span className="px-1.5 py-0.5 rounded-full bg-muted text-muted-foreground">
          {p.type === 'house' ? 'House' : 'Land'}
        </span>
        {p.area && <span className="px-1.5 py-0.5 rounded-full bg-muted text-muted-foreground">{p.area} m²</span>}
      </div>

      {p.noise != null && (
        <div className="flex items-center gap-1.5 mb-2">
          <span
            className="inline-block w-2.5 h-2.5 rounded-full"
            style={{ backgroundColor: ldenToColor(p.noise) }}
          />
          <span className="text-xs text-muted-foreground">
            Noise: <strong className="text-foreground">{p.noise.toFixed(1)} dB</strong>
          </span>
        </div>
      )}

      <a
        href={p.url}
        target="_blank"
        rel="noopener noreferrer"
        className="block text-center text-xs text-primary hover:underline py-1.5 border border-border rounded-md"
      >
        View on portal
      </a>
    </div>
  )
}
