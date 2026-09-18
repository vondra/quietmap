/** The popup's one plain line for emission layers the server answered without. */
import type { NoiseComputeData } from '../types/noise'

const VISITOR_WORDS: Record<NonNullable<NoiseComputeData['unavailable_layers']>[number], string> = {
  aircraft: 'aircraft',
  leisure: 'sports ground',
  ships: 'ship',
}

export function unavailableLayersSentence(layers: NoiseComputeData['unavailable_layers']): string | null {
  if (!layers?.length) return null
  const words = layers.map((layer) => VISITOR_WORDS[layer] ?? layer)
  const list = words.length === 1 ? words[0] : `${words.slice(0, -1).join(', ')} and ${words.at(-1)}`
  return `${list[0].toUpperCase()}${list.slice(1)} data is unavailable here; the level shown does not include it.`
}
