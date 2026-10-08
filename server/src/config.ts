// The server's configuration, read once from the environment at startup; server/README.md
// documents every variable. Locations always come from here, never from the code.
import { accessSync, constants, statSync } from 'node:fs'
import { join } from 'node:path'

export interface Config {
  host: string
  port: number
  /** The `qm-popup` executable. */
  popupBin: string
  /** The `qm-raster` executable, which draws the data layers. */
  rasterBin: string
  /** The prepared release: one directory per year beside the global tables. */
  preparedDir: string
  /** The years the release serves; the first answers a click that names none. */
  years: string[]
  /** Clicks computed at the same time; each uses every core. */
  popupConcurrency: number
  /** Heatmap tiles: the manifest `current.json` and the pmtiles archives it names. */
  tilesDir: string
  /** Every response carries `X-Robots-Tag: noindex` (a host search engines must skip). */
  noIndex: boolean
  /** The Photon geocoder that finds and names places: its base URL, the public one unless set. */
  photonUrl: string
}

function required(env: NodeJS.ProcessEnv, name: string): string {
  const value = env[name]
  if (!value) throw new Error(`${name} is not set (see server/README.md)`)
  return value
}

function positiveInteger(text: string, name: string, max: number): number {
  const value = Number(text)
  if (!/^[0-9]+$/.test(text) || value < 1 || value > max) {
    throw new Error(`${name} must be an integer from 1 to ${max}, not ${JSON.stringify(text)}`)
  }
  return value
}

function directory(path: string, name: string): string {
  if (!statSync(path, { throwIfNoEntry: false })?.isDirectory()) {
    throw new Error(`${name}: ${path} is not a directory`)
  }
  return path
}

/** Reads and checks the configuration: a missing binary or year directory stops the start. */
export function readConfig(env: NodeJS.ProcessEnv = process.env): Config {
  const popupBin = required(env, 'QM_POPUP_BIN')
  accessSync(popupBin, constants.X_OK)
  const rasterBin = required(env, 'QM_RASTER_BIN')
  accessSync(rasterBin, constants.X_OK)
  const preparedDir = directory(required(env, 'QM_PREPARED_DIR'), 'QM_PREPARED_DIR')
  const years = required(env, 'QM_YEARS').split(',').map(year => year.trim())
  for (const year of years) {
    if (!/^[0-9]{4}$/.test(year)) throw new Error(`QM_YEARS: ${JSON.stringify(year)} is not a year`)
    directory(join(preparedDir, year), 'QM_YEARS')
  }
  return {
    host: env.HOST || '127.0.0.1',
    port: positiveInteger(required(env, 'PORT'), 'PORT', 65535),
    popupBin,
    rasterBin,
    preparedDir,
    years,
    popupConcurrency: positiveInteger(env.QM_POPUP_CONCURRENCY || '2', 'QM_POPUP_CONCURRENCY', 64),
    tilesDir: directory(required(env, 'QM_TILES_DIR'), 'QM_TILES_DIR'),
    noIndex: env.QM_NOINDEX === '1',
    photonUrl: env.QM_PHOTON_URL || 'https://photon.komoot.io',
  }
}
