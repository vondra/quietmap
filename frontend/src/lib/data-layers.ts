// The data layers: the ground, obstacles and traffic every click computes over, and the forest it
// leaves out, drawn from the served release by `qm-raster`
// (`/api/raster/{id}/{z}/{x}/{y}.png`). Its zooms: the server's (server/src/routes/raster.ts).
export const DATA_LAYERS = [
  {
    id: 'elevation', label: 'Elevation', minzoom: 10, opacity: 0.75,
    tooltip: 'Ground height above sea level at every lattice node (1 arc-second), shaded by its slope: the hills that screen and the ground every ray crosses. Green low, yellow 300 m, red 800 m, purple 1,800 m.',
  },
  {
    id: 'forest', label: 'Forest', minzoom: 10, opacity: 0.8,
    tooltip: 'Tree cover at every lattice node: the darker the green, the denser the canopy. Trees take nothing off in the computation: CNOSSOS-EU, the EU method, has no term for foliage, and the ground under them counts as soft ground.',
  },
  {
    id: 'hard', label: 'Hard ground', minzoom: 10, opacity: 0.7,
    tooltip: 'Acoustically hard share of the ground at every lattice node, yellow little to red all: paved, built over or water reflects sound, open soil and grass absorb it (CNOSSOS ground factor G = 1 - hard share).',
  },
  {
    id: 'buildings', label: 'Buildings', minzoom: 13, opacity: 0.85,
    tooltip: 'Buildings as the computation screens and reflects with them, coloured by height: yellow 3 m, orange 8 m, red 15 m, dark red 25 m, purple 50 m and more.',
  },
  {
    id: 'traffic', label: 'Road traffic', minzoom: 13, opacity: 1,
    tooltip: 'Vehicles a day on every road: yellow 100, orange 5,000, red 15,000, dark red 40,000 and more. Solid where counted, half where a model makes them (mostly from the buildings around), faint where a fixed estimate per road class stands.',
  },
  {
    id: 'trains', label: 'Trains', minzoom: 11, opacity: 1,
    tooltip: 'Trains a day on every track, trams included: light blue 1, blue 10, dark blue 50, navy 150, purple 300 and more. Solid where timetables give all of them, half where they give some, faint where none do.',
  },
  {
    id: 'others', label: 'Other sources', minzoom: 12, opacity: 1,
    tooltip: 'The other sources every click computes, where it computes them (a dot per site, or per cell of an area): purple industry, teal wind turbines, gold church bells and calls to prayer, pink people outside bars and restaurants, green sport and play, slate car parks and street parking, blue ships (their density cells), grey the airports\' taxiways and runways, orange the horns sounded on the approach to level crossings (the United States and Canada). Buildings\' own sound (heat pumps, air conditioners, school yards, warehouses) is not drawn here: the Buildings layer shows the buildings.',
  },
  {
    id: 'barriers', label: 'Noise barriers', minzoom: 13, opacity: 1,
    tooltip: 'Noise barriers the computation screens with: OpenStreetMap and national inventories.',
  },
] as const

export type DataLayerId = (typeof DATA_LAYERS)[number]['id']

/** The layers a URL's `data=` names, in the panel's order; unknown names are dropped. */
export function parseDataLayers(text: string | null): DataLayerId[] {
  const named = new Set(text?.split(',') ?? [])
  return DATA_LAYERS.map(layer => layer.id).filter(id => named.has(id))
}

/** The map style's id of a data layer's raster source and layer. */
export const dataLayerStyleId = (id: DataLayerId) => `data-${id}`

/** The lowest data layer in a map style, if any is on: the heatmap draws beneath it, so the data
 *  layers show over the noise. */
export function lowestDataLayerId(layers: readonly { id: string }[] | undefined): string | undefined {
  const ids = new Set<string>(DATA_LAYERS.map(layer => dataLayerStyleId(layer.id)))
  return layers?.find(layer => ids.has(layer.id))?.id
}
