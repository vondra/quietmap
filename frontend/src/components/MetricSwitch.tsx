// The quantity the whole map shows, as two segments: Nden in sone or Lden in dB.
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
    <div role="radiogroup" aria-label="Show the noise in" className="mb-1 flex gap-0.5 rounded-md bg-muted p-0.5">
      {CHOICES.map(choice => (
        <button
          key={choice.metric}
          type="button"
          role="radio"
          aria-checked={metric === choice.metric}
          title={choice.tooltip}
          data-testid={`metric-${choice.metric}`}
          onClick={() => onChange(choice.metric)}
          className={`h-7 flex-1 rounded-[5px] text-sm font-medium ${metric === choice.metric
            ? 'bg-background text-foreground shadow-sm'
            : 'text-muted-foreground hover:text-foreground'}`}
        >
          {choice.label}
        </button>
      ))}
    </div>
  )
}
