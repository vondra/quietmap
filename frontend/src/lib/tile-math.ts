/// Web Mercator (EPSG:3857) slippy-tile math — the single source of truth shared
/// by the raster cell inspector and the raster overlay layers. `z` is the tile
/// zoom; tile (0,0) is the NW corner and y increases southward (XYZ/OSM order).

/** (lng, lat) → fractional tile coordinates at zoom `z` (no rounding). The integer
 *  part is the tile index; the fractional part is the position within that tile. */
export function lngLatToTileFloat(lng: number, lat: number, z: number): [number, number] {
  const n = 2 ** z
  const latRad = (lat * Math.PI) / 180
  const merc = Math.log(Math.tan(latRad) + 1 / Math.cos(latRad))
  return [((lng + 180) / 360) * n, ((1 - merc / Math.PI) / 2) * n]
}

/** (lng, lat) → the integer tile index containing the point at zoom `z`. */
export function lngLatToTile(lng: number, lat: number, z: number): { x: number; y: number } {
  const [xf, yf] = lngLatToTileFloat(lng, lat, z)
  return { x: Math.floor(xf), y: Math.floor(yf) }
}

/** Tile-X edge `x` → its west-edge longitude at zoom `z` (pass `x + 1` for east). */
export function tileXToLng(x: number, z: number): number {
  return (x / 2 ** z) * 360 - 180
}

/** Tile-Y edge `y` → its north-edge latitude at zoom `z` (pass `y + 1` for south). */
export function tileYToLat(y: number, z: number): number {
  return (Math.atan(Math.sinh(Math.PI * (1 - (2 * y) / 2 ** z))) * 180) / Math.PI
}
