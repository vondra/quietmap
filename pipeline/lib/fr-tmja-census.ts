/**
 * Cerema TMJA road-census parser (France) — the ONE reader of the two
 * `tmja-*.csv` releases behind `enrich-roads-fr.ts`.
 *
 * Turns each census section into an AADT class split (light/medium/heavy/moto)
 * anchored on the section's two Lambert-93 endpoints reprojected to WGS84.
 * Owns the three things the CSVs get wrong if read naively: CRLF headers, the
 * 2019 `ratio_PL` encoding, and what makes two rows the same section.
 */

import { parse } from 'csv-parse/sync'
import proj4 from 'proj4'

// Lambert-93 (EPSG:2154) → WGS84
proj4.defs('EPSG:2154', '+proj=lcc +lat_0=46.5 +lon_0=3 +lat_1=49 +lat_2=44 +x_0=700000 +y_0=6600000 +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +units=m +no_defs')
const toWGS84 = (x: number, y: number): [number, number] => {
  const [lon, lat] = proj4('EPSG:2154', 'WGS84', [x, y])
  return [lat, lon]
}

export interface CensusSection {
  route: string        // "A0001", "N0007"
  ref: string          // normalized: "A1", "N7"
  lat: number          // representative midpoint — display/log only
  lon: number
  coords: [number, number][]   // [start, end] as [lon, lat]; matched per-segment, not by midpoint
  tmja: number         // total AADT
  ratio_pl: number     // heavy vehicle share (0-1)
  aadt_light: number
  aadt_medium: number  // buses ~ 2% of PL
  aadt_heavy: number
  aadt_moto: number    // estimated ~1% of total
}

/** How a release encodes `ratio_PL`. 2024 is a plain percentage with a decimal
 *  comma. In 2019 an INTEGER means tenths of a percent (85 = 8.5 %) while a
 *  decimal value already means percent — the mixed encoding is documented by
 *  the Université Gustave Eiffel / Cerema analysis of this exact file
 *  (https://ame.gitpages.univ-eiffel.fr/tmja-2019-analysis/). Applying it caps
 *  our copy's heavy share at 48.8 %, matching the source; reading the 1,986
 *  integer-valued 2019 sections as percent (p50 109, max 989) would write a
 *  negative aadt_light on 898 of them. */
export type RatioPlEncoding = 'percent' | 'tenths-of-percent-when-integer'

export interface TmjaCsvFile {
  label: string
  csvText: string
  ratioPlEncoding: RatioPlEncoding
}

export interface TmjaFileCounters {
  label: string
  parsed: number
  skipped: number
  skippedDuplicateSection: number
  skippedZeroSplit: number
}

/** Heavy share used when the census leaves `ratio_PL` empty or implausible. */
const HEAVY_SHARE_FALLBACK = 0.12
/** Above this the field holds a vehicle COUNT, not a share: six 2024 rows on
 *  A0043 (6,285-12,724) and seven 2019 rows carry one. Zero is treated the
 *  same way — a section with measured traffic and no heavy vehicle at all is a
 *  missing value, not a measurement. */
const RATIO_PL_MAX_PLAUSIBLE = 500
/** Class fractions applied to every section: ~1 % motorcycles of the total,
 *  ~2 % buses of the heavy-vehicle count. */
const MOTO_FRACTION = 0.01
const BUS_FRACTION_OF_HEAVY = 0.02

/** Every column the parser reads. A missing one throws rather than silently
 *  becoming a missing measurement — reading `ratio_PL` through a CRLF-mangled
 *  header name is what put the flat 0.12 fallback on all 3,263 sections. */
const REQUIRED_TMJA_COLUMNS = ['route', 'TMJA', 'ratio_PL', 'cumulD', 'cumulF', 'xD', 'yD', 'xF', 'yF'] as const

function heavyShareFromRatioPl(raw: string, encoding: RatioPlEncoding): number | null {
  const token = raw.trim()
  const value = parseFloat(token.replace(',', '.'))
  if (!Number.isFinite(value) || value <= 0 || value > RATIO_PL_MAX_PLAUSIBLE) return null
  // The 2019 rule is about the written form: a token without a decimal
  // separator is tenths of a percent, one with a separator is percent (the
  // shipped file has 1,986 of the first kind, 1,075 of the second, none like
  // "12,0"). A decoded share above 100 % is a count that slipped under the
  // raw cap and would write a negative light-vehicle class.
  const integerFormatted = !/[.,]/.test(token)
  const percent = encoding === 'tenths-of-percent-when-integer' && integerFormatted ? value / 10 : value
  return percent > 100 ? null : percent / 100
}

/**
 * Parses the TMJA releases in the given order and keeps the FIRST record per
 * census section, so passing the newest release first makes it win.
 *
 * Section identity is the source's own — `route` plus the `cumulD`-`cumulF`
 * milestone range — unique inside each release (1,100 of 1,100 rows in 2024,
 * 3,741 of 3,741 in 2019) and stable across them. The previous key rounded a
 * section midpoint to 0.01° and dropped 1,578 of 4,841 sections, 594 of them
 * behind a neighbouring section of the SAME release: two 2024 records of A26
 * at cumul 132945-133038 and 133038-133666 share the cell (50.14, 3.13) yet
 * carry TMJA 18,403 and 21,382.
 */
export function parseTmjaFiles(files: readonly TmjaCsvFile[]): { sections: CensusSection[]; counters: TmjaFileCounters[] } {
  const sections: CensusSection[] = []
  const counters: TmjaFileCounters[] = []
  const seenSectionIdentities = new Set<string>()

  for (const file of files) {
    let headerNames: string[] = []
    // Both releases ship CRLF; csv-parse strips the record delimiter (`bom`
    // handles a byte-order mark, `trim` any stray padding), so `ratio_PL` can
    // no longer arrive as `ratio_PL\r` and read as an absent column.
    const records = parse(file.csvText, {
      delimiter: ';',
      columns: (header: string[]) => {
        headerNames = header.map((name) => name.trim())
        return headerNames
      },
      skip_empty_lines: true,
      bom: true,
    }) as Record<string, string>[]

    const missing = REQUIRED_TMJA_COLUMNS.filter((name) => !headerNames.includes(name))
    if (missing.length > 0) throw new Error(`${file.label}: TMJA CSV is missing column(s) ${missing.join(', ')}`)

    const counter: TmjaFileCounters = { label: file.label, parsed: 0, skipped: 0, skippedDuplicateSection: 0, skippedZeroSplit: 0 }
    counters.push(counter)

    for (const record of records) {
      const decimal = (name: string) => parseFloat((record[name] || '').replace(',', '.'))
      const route = record['route'] || ''
      const tmja = decimal('TMJA')
      const xD = decimal('xD'), yD = decimal('yD'), xF = decimal('xF'), yF = decimal('yF')

      if (!route || !tmja || !xD || !yD || xD < 100000) { counter.skipped++; continue }

      // Representative midpoint — display only. Matching uses the section's two
      // endpoints as a line, so the AADT boundary lands at the section end (the
      // real junction), not on the midpoint bisector.
      const mx = xF && yF ? (xD + xF) / 2 : xD
      const my = xF && yF ? (yD + yF) / 2 : yD
      const [lat, lon] = toWGS84(mx, my)

      if (lat < 41 || lat > 51.5 || lon < -5.5 || lon > 10) { counter.skipped++; continue }

      // Milestones name the section, read as numbers so a release that writes
      // "133038,0" still yields to one that wrote "133038"; a release without
      // them (none shipped so far) falls back on the endpoints.
      const cumulD = decimal('cumulD'), cumulF = decimal('cumulF')
      const milestones = Number.isFinite(cumulD) && Number.isFinite(cumulF) ? `${cumulD}:${cumulF}` : `${xD},${yD}:${xF},${yF}`
      const sectionIdentity = `${route}:${milestones}`
      if (seenSectionIdentities.has(sectionIdentity)) { counter.skippedDuplicateSection++; continue }
      seenSectionIdentities.add(sectionIdentity)

      const [latD, lonD] = toWGS84(xD, yD)
      const coords: [number, number][] = [[lonD, latD]]
      if (xF && yF) {
        const [latF, lonF] = toWGS84(xF, yF)
        coords.push([lonF, latF])
      }

      // Normalize route: "A0001" → "A1", "N0007" → "N7"
      const ref = route.replace(/^([A-Z])0*/, '$1')

      const ratioHV = heavyShareFromRatioPl(record['ratio_PL'] || '', file.ratioPlEncoding) ?? HEAVY_SHARE_FALLBACK
      const aadt_moto = Math.round(tmja * MOTO_FRACTION)
      const totalHV = Math.round(tmja * ratioHV)
      const aadt_medium = Math.round(totalHV * BUS_FRACTION_OF_HEAVY)
      const aadt_heavy = totalHV - aadt_medium
      const aadt_light = Math.round(tmja - totalHV - aadt_moto)

      // A TMJA under 0.5 rounds every class to zero (sum = round(tmja) exactly,
      // by construction above) — the #31.4 writer guard rejects an all-zero
      // payload under this MEASURED id, so skip and count rather than fabricate
      // a "surveyed as zero" claim.
      if (aadt_light + aadt_medium + aadt_heavy + aadt_moto === 0) {
        counter.skippedZeroSplit++
        continue
      }

      sections.push({ route, ref, lat, lon, coords, tmja: Math.round(tmja), ratio_pl: ratioHV, aadt_light, aadt_medium, aadt_heavy, aadt_moto })
      counter.parsed++
    }
  }
  return { sections, counters }
}
