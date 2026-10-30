// The quantity the whole map shows, a row of the layer panel's Advanced group: Nden in sone or Lden in
// dB, as two segments at the row's end.
import { Gauge } from 'lucide-react'
import type { Metric } from '../lib/hm3-decoder'

const CHOICES: { metric: Metric; label: string; tooltip: string }[] = [
  {
    metric: 'nden',
    label: 'Sone',
    tooltip: 'How loud a place sounds over the whole day: twice the sone sounds twice as loud',
  },
  {
    metric: 'lden',
    label: 'Lden dB',
    tooltip: 'The EU\'s day-evening-night level: the sound\'s energy over the day, the evening 5 dB and the night 10 dB louder',
  },
]

export default function MetricSwitch({ metric, onChange }: { metric: Metric; onChange: (metric: Metric) => void }) {
  return (
    <div className="flex items-center gap-2 py-1 px-1">
      <Gauge className="size-3.5 shrink-0 text-muted-foreground" />
      <span className="flex-1 text-xs text-muted-foreground">Noise in</span>
      <div role="radiogroup" aria-label="Show the noise in" className="flex gap-0.5 rounded-md bg-muted p-0.5">
        {CHOICES.map(choice => (
          <button
            key={choice.metric}
            type="button"
            role="radio"
            aria-checked={metric === choice.metric}
            title={choice.tooltip}
            data-testid={`metric-${choice.metric}`}
            onClick={() => onChange(choice.metric)}
            className={`h-6 rounded-[5px] px-2 text-xs font-medium ${metric === choice.metric
              ? 'bg-background text-foreground shadow-sm'
              : 'text-muted-foreground hover:text-foreground'}`}
          >
            {choice.label}
          </button>
        ))}
      </div>
    </div>
  )
}
