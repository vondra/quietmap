/**
 * Central registry of data sources used to enrich roads, railways, buildings,
 * and industrial layers. Each row in an Arrow file carries a `dataset_id`
 * pointing back here, so the popup can show the user which dataset contributed
 * the data.
 *
 * Conventions:
 *   - `id` is globally unique, monotonically assigned, NEVER recycled.
 *     Allocate with `npx tsx pipeline/lib/allocate-dataset-id.ts <layer> <key>`.
 *   - `key` is a stable slug; also unique.
 *   - `priority` = how authoritative the source is for the area it covers:
 *       80  national authority    (per-country census, cadastre, pollutant registry)
 *       70  continental / multi-country measured
 *           (E-PRTR EU-wide, EU city traffic 36 cities, GTFS feeds)
 *       50  global measured baseline, coarse but worldwide
 *           (Overture buildings, Copernicus DEM, GPPD power plants)
 *       10  heuristic / synthetic inference
 *       0   legacy sentinel ("unspecified")
 *     Higher priority wins. Ties broken by higher id (deterministic).
 *   - ID = 0 is reserved for "unspecified" — pre-provenance legacy rows.
 *
 * Source IDs are generated once and kept stable so persisted rows remain readable.
 */

/**
 * How authoritative a dataset is — the rank driver for `shouldOverwrite`.
 * Defined HERE (upstream of `sources.ts`, which imports this module) so an
 * entry can DECLARE its provenance when the priority-ladder derivation in
 * `sources.ts::provenanceFromEntry` would misfile it. Full tier semantics:
 * `sources.ts` docstrings.
 */
export type Provenance =
  | 'city-measured'
  | 'national-measured'
  | 'continental-measured'
  | 'global-measured'
  | 'national-proxy'
  | 'heuristic'
  | 'baseline'
  | 'none'

export interface Dataset {
  id: number
  layer: 'roads' | 'railways' | 'buildings' | 'industrial' | 'aircraft' | 'any'
  key: string
  name: string
  year: number | null
  license: string | null
  url: string | null
  priority: number
  /** Declared provenance tier — overrides the priority-ladder derivation in
   *  `sources.ts::provenanceFromEntry`. Use when the ladder position lies
   *  about the real rank (Overture/Copernicus are baselines at global-tier
   *  priority; the R7 taper is a baseline at heuristic-tier priority so every
   *  real enricher overwrites it). Absent = derive from `priority`. */
  provenance?: Provenance
  /** Road classes (engine inputs.rs codes: 0 motorway..4 tertiary, 10-12 links)
   *  this source may stamp — mirrors the `coverage` set its enricher passes to
   *  `writeRoadAadt`. Absent = the enricher declares no class gate; the
   *  invariant scanner (`audit-enrichment-invariants.ts`) then skips the
   *  coverage check for rows carrying this id. */
  roadCoverage?: readonly number[]
  /** What the enricher actually STAMPS under this id (C3.1 v2, accepted /gg):
   *  'counted' = sensor/census counts only; 'derived' = mixes counted with
   *  class/surface proxies under one id (per-row provenance indistinguishable);
   *  'proxy' = no counts published, classification-based values only.
   *  Consumers: traffic-defaults generator ingests counted ONLY; the A3
   *  scanner enforces that proxy ids never feed it. Absent = not yet judged
   *  (treated as not-counted). */
  measurement?: 'counted' | 'derived' | 'proxy'
  /** Rail families whose MEASURED (GTFS/timetable) counts this source may
   *  stamp: 'rail' = rail_type 0, 'tram' = rail_type 1/2. Per-family class
   *  defaults (≤250 trains/day) are family-correct by construction and not
   *  limited by this. Absent = capability unknown; the scanner skips the
   *  tram-overcount check for this id. */
  railFamilies?: readonly ('rail' | 'tram')[]
  /** Motorcycle-dominant traffic mix is genuine for this source's country
   *  (ID, PH, …) — the invariant scanner's moto-scramble rule (R2) would
   *  otherwise flag every row; it skips ids declaring this. */
  highMoto?: boolean
  /** Divisors on rows stamped by this source are computed by the graph-walk's
   *  lateral parallel-track spread (`rail-graph-metrics.ts::applyParallelSpread`,
   *  driven by `rail-walk-enrich.ts`) — the generic token-grouped only-raise
   *  pass (`enrich-railways-parallel.ts`) must never override them, including
   *  raising a divisor the walk deliberately set to 1. Absent = the generic
   *  pass owns this source's divisors as usual. */
  railDivisorFromWalk?: true
  /** Single-country ownership for a source whose PROVENANCE TIER alone does
   *  not already say so (DE Step A v2 Codex review item 4, 2026-07-16):
   *  `cz-timetable-silent` is baseline-tier — a residual claim, not a
   *  measurement — yet only the CZ enricher may stamp or overwrite it (the
   *  claim "CZPTT proves no scheduled service here" is national testimony a
   *  foreign feed cannot refute). City/national tiers are nationally owned
   *  by construction and never need this flag. Consumed by
   *  `sources.ts::isNationallyOwnedSource` → the rail-walk driver's
   *  foreign-national stamp guard (`rail-walk-enrich.ts`). */
  nationallyOwned?: true
}

/** Classes 0-4 + links — the standard major-road census coverage every
 *  coverage-passing national road enricher uses today (fi/sa/pl/dk/no/us/nz). */
const MAJOR_ROAD_COVERAGE = [0, 1, 2, 3, 4, 10, 11, 12] as const

export const DATASETS: Dataset[] = [
  // ── Sentinel ──
  {
    id: 0,
    layer: 'any',
    key: 'unspecified',
    name: 'Unspecified / pre-provenance legacy',
    year: null,
    license: null,
    url: null,
    priority: 0,
  },

  // ── Aircraft: ADS-B observational ──
  // Single aircraft source today — `aircraft-extract` produces all
  // three popup arrows (airborne / cruise / ground). The Rust mirror
  // exposes this entry's id as `AIRCRAFT_ADSB_SOURCE_ID` so callers can
  // use a const, not a dynamic lookup.
  {
    id: 1,
    layer: 'aircraft',
    key: 'global-adsb-planet',
    name: 'ADSBexchange planet TAR archives',
    year: 2024,
    license: 'CC-BY-SA-4.0',
    url: 'https://www.adsbexchange.com/',
    priority: 50,
  },

  // ── Roads: continental ──
  {
    id: 10,
    layer: 'roads',
    key: 'eu-city-traffic',
    name: 'EU Harmonized Traffic Volumes (Nature Scientific Data)',
    year: 2023,
    license: 'CC-BY-4.0',
    url: 'https://github.com/XavB64/traffic-volume-data-EU-cities',
    priority: 70,
    measurement: 'counted',
  },
  {
    id: 11,
    layer: 'roads',
    key: 'service-tree-heuristic',
    name: 'Service-tree residential flow heuristic',
    year: 2025,
    license: 'project-internal',
    url: null,
    priority: 10,
    measurement: 'derived',
    // The split is COMPUTED from one total via the per-country fleet table
    // (country-fleet.generated.ts, up to 45 % moto in SE Asia) — a loader
    // column-scramble is structurally impossible here, so R2's cars-in-the-
    // moto-column tripwire must not fire on honest high-moto countries.
    // Split correctness is pinned by direct unit tests instead
    // (enrich-roads-service-tree.test.ts splitAADT conservation/clamp).
    highMoto: true,
  },
  {
    // Junction-bounded same-ref continuity fill: copies a MEASURED neighbour's
    // AADT across an unmeasured major-road segment of the same road when no
    // junction lets traffic leave between them (flow conservation). Heuristic
    // tier (priority 10) so any real measurement still wins and the engine
    // re-applies access_factor. See enrich-roads-continuity-fill.ts.
    id: 12,
    layer: 'roads',
    key: 'road-continuity-heuristic',
    name: 'Same-ref continuity fill (junction-bounded)',
    year: 2026,
    license: 'project-internal',
    url: null,
    priority: 10,
    measurement: 'derived',
    // Major roads only (0-4 + links) — the fill must never stamp a local
    // street; the scanner's coverage invariant (R1) enforces it per-row.
    roadCoverage: MAJOR_ROAD_COVERAGE,
  },
  {
    // R7 transition taper (owner 2026-07-10): grades speed/AADT steps between
    // adjacent same-road segments with no junction between them — a car
    // decelerates over distance, it does not step. Writes ONLY onto rows with
    // source_id=0 AND untagged speed; declared BASELINE so every real
    // enricher (census, continental, service-tree, continuity) freely
    // overwrites taper rows on its next run; the taper pass runs last in the chain.
    id: 9862,
    layer: 'roads',
    key: 'osm-transition-taper',
    name: 'Transition taper (graded junction-free speed/AADT steps)',
    year: 2026,
    license: 'project-internal',
    url: null,
    priority: 10,
    provenance: 'baseline',
    measurement: 'derived',
    // Through-traffic classes only — local streets (5-8) carry values too
    // small for a visible cliff; links are junction-adjacent by nature.
    // 0/1 excluded is CZ-v1 scoping (census covers them there) — revisit
    // per-country when the taper widens past CZ (NAPI resolution export).
    roadCoverage: [2, 3, 4, 9],
  },
  {
    // Timetable-silent residual (owner decision 2026-07-11): in a country
    // whose rail enricher has full national timetable coverage, a line the timetable
    // does NOT know gets a small explicit residual (2 pax + 1 frt/day —
    // occasional special/freight runs) instead of the engine branch default
    // (30+5). Dead branches drop ~9 dB out of the red (Trať 162: 75→66 dB)
    // without going silent. BASELINE rank: any real measurement overwrites;
    // explicit frt=1 stops the engine's per-column zero-defaulting from
    // re-adding 5 freight.
    // "Silence" is a NATIONAL claim — CZPTT can only testify about Czech
    // track — so the key is cc-prefixed like every national feed: that is
    // what makes heal-rail-country-bleed sweep its foreign residue (the
    // un-prefixed original stamped 168,749 rows on DE/AT/PL/SK track inside
    // the CZ pass bbox; #31 CZ finding). Each future silent-capable country
    // gets its OWN cc-keyed id, never a shared one (#26B).
    id: 9863,
    layer: 'railways',
    key: 'cz-timetable-silent',
    name: 'CZ timetable-silent residual (CZPTT has no scheduled service)',
    year: 2026,
    license: 'project-internal',
    url: null,
    priority: 10,
    provenance: 'baseline',
    measurement: 'derived',
    railFamilies: ['rail'],
    railDivisorFromWalk: true,
    // Baseline tier yet CZ-owned (see the field's doc): the rail-walk
    // foreign-national guard must protect these rows from a neighbour's
    // higher-rank walk (verified live: DE's widened Step A v2 scope reaches
    // border rows carrying this id, and shouldOverwrite(baseline,
    // national-measured) alone would hand them to DE).
    nationallyOwned: true,
  },

  // ── Roads: national ──
  {
    id: 20,
    layer: 'roads',
    key: 'cz-rsd-scitani',
    name: 'ŘSD Celostátní sčítání dopravy',
    year: 2020,
    license: 'CC-BY-4.0',
    url: 'https://geoportal.rsd.cz/',
    priority: 80,
    measurement: 'counted',
    roadCoverage: [0, 1, 2, 3, 4, 10, 11, 12], // mirrors RSD_COVERAGE in enrich-roads-cz.ts (R1b bucket A)
  },
  {
    id: 9003,
    layer: 'roads',
    key: 'city-praha-tsk',
    name: 'TSK Praha — Intenzity dopravy',
    year: 2025,
    license: 'Prague open data (no explicit CC; city portal terms)',
    url: 'https://tsk-praha.cz/',
    priority: 90,
    measurement: 'derived',
    /* Working-day 0–24 h totals per monitored-network section (954 profiles,
     * 415 streets, whole administrative Prague; mix of loop detectors, mobile
     * counters and manual surveys). 'derived', not 'counted': values are
     * WORKING-DAY 0-24h totals — TSK's TP-189 weekday→AADT expansion table
     * is not published with the file, so the unexpanded proxy
     * must not enter counted-only consumers (~+5-8% vs true AADT,
     * conservative). Matched inside the Prague ADM2 polygon. */
    roadCoverage: [1, 2, 3, 4, 5, 11, 12], // mirrors city-datasets.ts coverage (R1b bucket B)
  },
  {
    id: 9004,
    layer: 'roads',
    key: 'city-wien-dauerzaehlstellen',
    name: 'Stadt Wien MA46 — Dauerzählstellen DTV',
    year: 2025,
    license: 'CC-BY-4.0',
    url: 'https://www.data.gv.at/katalog/dataset/stadt-wien_verkehrszhlstellenzhlwertewien',
    priority: 90,
    measurement: 'counted',
    /* ~70 permanent automatic counters (loops + side radar) on Vienna's
     * B + G street network, monthly Mon–Sun all-days DTV per station and
     * direction since 2016. 'counted': full-year, all-day-type means from
     * fixed sensors — MA46 publishes the day-type-correct monthly DTVMS,
     * the adapter only takes the days-weighted mean of the latest complete
     * non-pandemic year (2025; the adapter logs + caches the year it
     * picked). No proxy rows, no gap-fill. Matched by station point
     * proximity inside the Wien(Stadt) ADM2 polygon. */
    roadCoverage: [1, 2, 3, 4, 5, 11, 12], // mirrors city-datasets.ts coverage (R1b bucket B)
  },
  {
    id: 9005,
    layer: 'roads',
    key: 'city-brno-detectors',
    name: 'Brno BKOM — pentlogram intenzity dopravy',
    year: 2023,
    license: 'CC-BY-SA-4.0',
    url: 'https://data.brno.cz/datasets/intenzita-dopravy-intenzita-vozidel-vehicle-traffic-intensity',
    priority: 90,
    measurement: 'derived',
    /* 589 section polylines over Brno's main street graph, per-edition
     * vehicles/24 h (latest edition 2023). 'derived', not 'counted':
     * values are working-day-period intensities quantized to whole
     * THOUSANDS with a PERCENT heavy share (buses folded into trucks),
     * and pentlogram editions carry sections forward between BKOM survey
     * campaigns — counted vs interpolated sections are indistinguishable
     * per row, which is exactly the plan-§2.6 "mixed under one id" case.
     * Matched by section-line proximity inside the Brno-City ADM2
     * polygon. */
    roadCoverage: [1, 2, 3, 4, 5, 9, 11, 12], // mirrors city-datasets.ts coverage (R1b bucket B)
  },
  {
    id: 21,
    layer: 'roads',
    key: 'us-fhwa-hpms',
    name: 'FHWA Highway Performance Monitoring System',
    year: 2022,
    license: 'public-domain',
    url: 'https://www.fhwa.dot.gov/policyinformation/hpms.cfm',
    priority: 80,
    measurement: 'counted',
    roadCoverage: MAJOR_ROAD_COVERAGE,
  },
  {
    id: 22,
    layer: 'roads',
    key: 'de-bast-autobahn',
    name: 'BASt SVZ Autobahnen',
    year: 2021,
    license: 'DL-DE BY 2.0',
    url: 'https://www.bast.de/',
    priority: 80,
    measurement: 'counted',
  },
  {
    id: 23,
    layer: 'roads',
    key: 'de-bast-bundesstrassen',
    name: 'BASt SVZ Bundesstraßen',
    year: 2021,
    license: 'DL-DE BY 2.0',
    url: 'https://www.bast.de/',
    priority: 80,
    measurement: 'counted',
  },
  {
    id: 24,
    layer: 'roads',
    key: 'fr-cerema-tmja',
    name: 'Cerema Trafic Moyen Journalier Annuel',
    year: 2024,
    license: 'etalab-2.0',
    url: 'https://www.data.gouv.fr/',
    priority: 80,
    measurement: 'counted',
  },

  // ── Railways: continental ──
  {
    id: 100,
    layer: 'railways',
    key: 'global-gtfs-transit',
    name: 'Continental GTFS transit feeds (aggregated; mostly Europe + AU/IN)',
    year: 2025,
    license: 'mixed (per-operator)',
    url: null,
    priority: 70,
    railFamilies: ['rail', 'tram'], // family-aware Variant B (enrich-railway-europe.ts)
  },

  // ── Railways: national ──
  {
    id: 110,
    layer: 'railways',
    key: 'cz-szcd-gtfs',
    name: 'Správa železnic GTFS',
    year: 2025,
    license: 'CC-BY-4.0',
    url: 'https://www.spravazeleznic.cz/',
    priority: 80,
    railFamilies: ['rail'], // CZPTT is heavy-rail only; trams get class defaults
    railDivisorFromWalk: true,
  },

  // ── Buildings: national ──
  {
    id: 200,
    layer: 'buildings',
    key: 'cz-ruian-vfr',
    name: 'ČÚZK RÚIAN VFR',
    year: 2024,
    license: 'CC-BY-4.0',
    url: 'https://www.cuzk.cz/Uvod/Produkty-a-sluzby/RUIAN/',
    priority: 80,
  },
  {
    id: 201,
    layer: 'buildings',
    key: 'es-catastro',
    name: 'Dirección General del Catastro',
    year: 2024,
    license: 'CC-BY-4.0',
    url: 'https://www.catastro.minhap.es/',
    priority: 80,
  },

  // ── Industrial: global ──
  {
    id: 300,
    layer: 'industrial',
    key: 'global-gppd',
    name: 'Global Power Plant Database',
    year: 2021,
    license: 'CC-BY-4.0',
    url: 'https://datasets.wri.org/dataset/globalpowerplantdatabase',
    priority: 50,
  },
  {
    id: 301,
    layer: 'industrial',
    key: 'global-uswtdb',
    name: 'US Wind Turbine Database',
    year: 2024,
    license: 'public-domain',
    url: 'https://eerscmap.usgs.gov/uswtdb/',
    priority: 80,
  },
  // GEM heavy-industry trackers — global per-facility asset locations with a
  // real sector (steel / cement / coal mine), the public map GeoJSON served
  // from GEM's DigitalOcean CDN (CC-BY-4.0). priority 50 → 'global-measured',
  // same tier as GPPD: a real per-facility sector match, not an OSM guess.
  {
    // ids 331-333 (> the 330 national-mix baseline): specific steel/cement/coal NACE
    // must win the equal-rank/year id-tiebreak over the generic power-mix (gg 2026-06-14;
    // Codex+Gemini consensus — at 302-304 the generic 330 overrode them).
    id: 331,
    layer: 'industrial',
    key: 'global-gem-steel',
    name: 'GEM Global Iron and Steel Tracker',
    year: 2025,
    license: 'CC-BY-4.0',
    url: 'https://globalenergymonitor.org/projects/global-iron-and-steel-tracker/',
    priority: 50,
  },
  {
    id: 332,
    layer: 'industrial',
    key: 'global-gem-cement',
    name: 'GEM Global Cement and Concrete Tracker',
    year: 2025,
    license: 'CC-BY-4.0',
    url: 'https://globalenergymonitor.org/projects/global-cement-and-concrete-tracker/',
    priority: 50,
  },
  {
    id: 333,
    layer: 'industrial',
    key: 'global-gem-coalmine',
    name: 'GEM Global Coal Mine Tracker',
    year: 2025,
    license: 'CC-BY-4.0',
    url: 'https://globalenergymonitor.org/projects/global-coal-mine-tracker/',
    priority: 50,
  },

  // ── Industrial: regional / continental ──
  {
    id: 310,
    layer: 'industrial',
    key: 'europe-eprtr',
    name: 'European Pollutant Release and Transfer Register',
    year: 2022,
    license: 'CC-BY-4.0',
    url: 'https://industry.eea.europa.eu/',
    priority: 70,
  },

  // ── Industrial: national ──
  {
    id: 320,
    layer: 'industrial',
    key: 'cz-irz',
    name: 'ČHMÚ Integrovaný registr znečišťování',
    year: 2023,
    license: 'CC-BY-4.0',
    url: 'https://www.irz.cz/',
    priority: 80,
  },

  // ── Industrial: global baseline (plan v5 D.1, collapses 125 `{iso}-industrial` keys) ──
  // Any per-country `enrich-industrial-{iso}.ts` that merely stamps OSM
  // tags + a sprinkle of GEM / GPPD / USWTDB hits writes this id now. A
  // row with id=330 is not "national-measured" — it is an OSM-derived
  // baseline. Real national industrial registries keep their own id
  // (e.g. id=320 cz-irz).
  {
    id: 330,
    layer: 'industrial',
    key: 'global-industrial-national-mix',
    name: 'OSM-derived per-country industrial (may include GEM/GPPD/USWTDB matches)',
    year: 2025,
    license: 'ODbL-1.0',
    url: 'https://www.openstreetmap.org/',
    // priority 50 → 'global-measured': the NACE is a real per-facility fuel match
    // (GEM/GPPD/USWTDB), so it must outrank the name-keyword heuristic (id 9000,
    // priority 10 → 'heuristic'), which otherwise won the id tiebreak at equal rank.
    priority: 50,
  },

  // ── Roads: national (OSM-only) ──
  {
    id: 1004,
    layer: 'roads',
    key: 'ar-national-roads',
    name: 'DNV TMDA 2017-18 + IGN GeoServer',
    year: 2018,
    license: 'public-data',
    url: 'https://ide.transporte.gob.ar/geoserver/observ/ows',
    priority: 80,
    measurement: 'derived',
  },
  {
    id: 1013,
    layer: 'roads',
    key: 'bo-national-roads',
    name: 'ABC Red Vial Fundamental (community ArcGIS mirror)',
    year: 2024,
    license: 'community-mirror',
    url: 'https://services2.arcgis.com/1GTOs4RWV6SKu0wr/',
    priority: 80,
    measurement: 'proxy',
  },
  {
    id: 1014,
    layer: 'roads',
    key: 'br-national-roads',
    name: 'DNIT Rodovias Federais',
    year: 2017,
    license: 'public-data',
    url: 'https://www.dnit.gov.br/',
    priority: 80,
    measurement: 'proxy',
  },
  {
    id: 1019,
    layer: 'roads',
    key: 'ca-national-roads',
    name: 'Quebec MTQ DJMA',
    year: 2024,
    license: 'CC-BY-4.0',
    url: 'https://www.donneesquebec.ca/recherche/dataset/debit-de-circulation',
    priority: 80,
    measurement: 'counted',
    roadCoverage: [0, 1, 2, 3, 4, 10, 11, 12], // mirrors MTQ_COVERAGE in enrich-roads-ca.ts (R1b bucket A)
  },
  {
    id: 1023,
    layer: 'roads',
    key: 'cl-national-roads',
    name: 'MOP Vialidad Red Vial + Plan Nacional de Censos TMDA',
    year: 2025,
    license: 'public-data',
    url: 'https://rest-sit.mop.gob.cl/',
    priority: 80,
    measurement: 'derived',
  },
  {
    id: 1025,
    layer: 'roads',
    key: 'cn-national-roads',
    name: 'China Highway Network (ArcGIS mirror) + tier defaults',
    year: 2024,
    license: 'community-mirror',
    url: 'https://services1.arcgis.com/ERdCHt0sNM6dENSD/',
    priority: 80,
    measurement: 'proxy',
  },
  {
    id: 1026,
    layer: 'roads',
    key: 'co-national-roads',
    name: 'INVIAS Red Vial Nacional + TPDS_NUBE TPDA',
    year: 2024,
    license: 'public-data',
    url: 'https://services6.arcgis.com/kyerLIHvrND0OSya/',
    priority: 80,
    measurement: 'derived',
  },
  {
    id: 1031,
    layer: 'roads',
    key: 'dk-national-roads',
    name: 'Vejdirektoratet Mastra',
    year: 2025,
    license: 'CC-BY-4.0',
    url: 'https://www.opendata.dk/vejdirektoratet/taellinger-nogletal-mastra',
    priority: 80,
    measurement: 'counted',
    roadCoverage: MAJOR_ROAD_COVERAGE,
  },
  {
    id: 1034,
    layer: 'roads',
    key: 'ec-national-roads',
    name: 'CONGOPE Red Vial Ecuador (ArcGIS mirror)',
    year: 2024,
    license: 'public-data',
    url: 'https://services6.arcgis.com/pYn2F4v1aESZqj1u/',
    priority: 80,
    measurement: 'proxy',
  },
  {
    id: 1037,
    layer: 'roads',
    key: 'es-national-roads',
    name: 'MITMA Mapa de Tráfico',
    year: 2022,
    license: 'CC-BY-4.0',
    url: 'https://mapatrafico.transportes.gob.es/',
    priority: 80,
    measurement: 'counted',
  },
  {
    id: 1039,
    layer: 'roads',
    key: 'fi-national-roads',
    name: 'Väylävirasto Liikennemäärät',
    year: 2024,
    license: 'CC-BY-4.0',
    url: 'https://avoindata.suomi.fi/data/fi/dataset/liikennemaarat',
    priority: 80,
    measurement: 'counted',
    roadCoverage: MAJOR_ROAD_COVERAGE,
  },
  {
    id: 1041,
    layer: 'roads',
    key: 'gb-national-roads',
    name: 'DfT AADF (Annual Average Daily Flow)',
    year: 2024,
    license: 'Open Government Licence v3.0',
    url: 'https://roadtraffic.dft.gov.uk/',
    priority: 80,
    measurement: 'counted',
  },
  {
    id: 1048,
    layer: 'roads',
    key: 'id-national-roads',
    name: 'Bina Marga GIS (PUPR) — Jalan Daerah + Nasional + Tol',
    year: 2024,
    license: 'public-data',
    url: 'https://gisportal.binamarga.pu.go.id/',
    priority: 80,
    measurement: 'derived',
    highMoto: true,
  },
  {
    id: 1049,
    layer: 'roads',
    key: 'ie-national-roads',
    name: 'TII Counter Sites + Daily Class Aggregate',
    year: 2020,
    license: 'CC-BY-4.0',
    url: 'https://data.tii.ie/',
    priority: 80,
    measurement: 'counted',
  },
  {
    id: 1050,
    layer: 'roads',
    key: 'in-national-roads',
    name: 'Bharatmala Road Network (Esri Living Atlas India)',
    year: 2024,
    license: 'public-data',
    url: 'https://livingatlas.esri.in/',
    priority: 80,
    measurement: 'proxy',
    highMoto: true, // India rides two-wheelers — moto genuinely exceeds 0.5×cars, so R2 must not flag it (enrich-roads-in.ts sets up to 40% moto)
  },
  {
    id: 1054,
    layer: 'roads',
    key: 'it-national-roads',
    name: 'Anas TGM (Traffico Giornaliero Medio)',
    year: 2024,
    license: 'CC-BY-4.0',
    url: 'https://www.stradeanas.it/',
    priority: 80,
    measurement: 'counted',
  },
  {
    id: 1088,
    layer: 'roads',
    key: 'no-national-roads',
    name: 'NVDB Trafikkmengde (Statens vegvesen)',
    year: 2025,
    license: 'NLOD 2.0',
    url: 'https://nvdbapiles.atlas.vegvesen.no/',
    priority: 80,
    measurement: 'counted',
    roadCoverage: MAJOR_ROAD_COVERAGE,
  },
  {
    id: 1090,
    layer: 'roads',
    key: 'nz-national-roads',
    name: 'NZTA Carriageway + Auckland Transport AADT',
    year: 2024,
    license: 'CC-BY-4.0',
    url: 'https://opendata-nzta.opendata.arcgis.com/',
    priority: 80,
    measurement: 'counted',
    roadCoverage: MAJOR_ROAD_COVERAGE,
  },
  {
    id: 1093,
    layer: 'roads',
    key: 'pe-national-roads',
    name: 'MTC Provías Red Vial Nacional + dIMD (ArcGIS mirror)',
    year: 2024,
    license: 'public-data',
    url: 'https://services6.arcgis.com/G8JFnqCHKQ9vb8YW/',
    priority: 80,
    measurement: 'derived',
  },
  {
    id: 1095,
    layer: 'roads',
    key: 'ph-national-roads',
    name: 'DPWH Road Classification (ArcGIS)',
    year: 2026,
    license: 'public-data',
    url: 'https://services1.arcgis.com/IwZZTMxZCmAmFYvF/',
    priority: 80,
    measurement: 'proxy',
    highMoto: true,
  },
  {
    id: 1097,
    layer: 'roads',
    key: 'pl-national-roads',
    name: 'GDDKiA Generalny Pomiar Ruchu (GPR)',
    year: 2021,
    license: 'CC-BY-4.0',
    url: 'https://www.gov.pl/web/gddkia/generalny-pomiar-ruchu-20202021',
    priority: 80,
    measurement: 'counted',
    roadCoverage: MAJOR_ROAD_COVERAGE,
  },
  {
    id: 1098,
    layer: 'roads',
    key: 'py-national-roads',
    name: 'MOPC Rutas Nacionales (KMZ)',
    year: 2023,
    license: 'public-data',
    url: 'https://www.mopc.gov.py/red-vial/',
    priority: 80,
    measurement: 'proxy',
  },
  {
    id: 1102,
    layer: 'roads',
    key: 'sa-national-roads',
    name: 'MoT count stations + Riyadh PMS + SA Interactive Atlas',
    year: 2024,
    license: 'open-data',
    url: 'https://mot.gov.sa/en/open-data',
    priority: 80,
    measurement: 'derived',
    roadCoverage: MAJOR_ROAD_COVERAGE,
  },
  {
    id: 1113,
    layer: 'roads',
    key: 'th-national-roads',
    name: 'DRR Rural Roads AADT (MOT CKAN mirror) + DOH defaults',
    year: 2024,
    license: 'public-data',
    url: 'https://datagov.mot.go.th/',
    priority: 80,
    measurement: 'derived',
    highMoto: true, // Thailand is motorcycle-dominant — R2 must not flag its real moto share
  },
  {
    id: 1124,
    layer: 'roads',
    key: 've-national-roads',
    name: 'VE360 Vialidad (SIGOT community mirror)',
    year: 2019,
    license: 'community-mirror',
    url: 'https://services6.arcgis.com/lpJCO3ug8HhNiEOV/',
    priority: 80,
    measurement: 'proxy',
  },
  {
    // 9000 + ISO-3166 numeric (RU = 643), matching the ng/eg OSM-only batch.
    id: 9643,
    layer: 'roads',
    key: 'ru-national-roads',
    name: 'Russia-tuned CNOSSOS class defaults (no open per-segment AADT)',
    year: 2024,
    license: 'derived-from-OSM',
    url: null,
    priority: 80,
    // No published per-segment counts — country-tuned class defaults anchored to
    // AUTOSTAT/Avtodor/Rosstat aggregates (same shape as CN/IN, ids 1025/1050).
    measurement: 'proxy',
    roadCoverage: [0, 1, 2, 3, 4, 5, 10, 11, 12], // mirrors RU_COVERAGE in enrich-roads-ru.ts (R1b bucket A)
  },
  {
    id: 9566, // NG = 566
    layer: 'roads',
    key: 'ng-national-roads',
    name: 'Nigeria-tuned CNOSSOS class defaults (FRSC/FERMA portals unavailable)',
    year: 2024,
    license: 'derived-from-OSM',
    url: null,
    priority: 80,
    measurement: 'proxy',
    roadCoverage: [0, 1, 2, 3, 4, 5, 10, 11, 12], // mirrors NG_COVERAGE in enrich-roads-ng.ts (R1b bucket A)
    highMoto: true, // "Okada" motorcycle taxis + "keke" tricycles dominate — enrich-roads-ng.ts sets 35% moto by design; R2 must not flag it
  },
  {
    id: 9818, // EG = 818
    layer: 'roads',
    key: 'eg-national-roads',
    name: 'Egypt-tuned CNOSSOS class defaults (no open per-segment AADT)',
    year: 2024,
    license: 'derived-from-OSM',
    url: null,
    priority: 80,
    measurement: 'proxy',
  },
  {
    id: 9392, // JP = 392
    layer: 'roads',
    key: 'jp-national-roads',
    name: 'Japan MLIT Road Traffic Census R3 (2021) name/ref join',
    year: 2021,
    license: 'derived-from-OSM',
    url: 'https://www.mlit.go.jp/road/census/r3/',
    priority: 80,
    measurement: 'counted',
    roadCoverage: [0, 1, 2, 3, 4, 10, 11, 12], // mirrors JP_COVERAGE in enrich-roads-jp.ts (R1b bucket A)
  },
  {
    id: 9865,
    layer: 'roads',
    key: 'jp-class-median-fallback',
    name: 'Japan MLIT R3 class-median fallback (census-derived, no geometry join)',
    year: 2021,
    license: 'derived-from-OSM',
    url: 'https://www.mlit.go.jp/road/census/r3/',
    priority: 80,
    measurement: 'proxy',
    roadCoverage: [0, 1, 2, 3, 4, 10, 11, 12],
  },
  {
    id: 9484, // MX = 484
    layer: 'roads',
    key: 'mx-national-roads',
    name: 'Mexico SICT/IMT Datos Viales 2025 (200m polyline match)',
    year: 2025,
    license: 'derived-from-OSM',
    url: null,
    priority: 80,
    measurement: 'counted',
    roadCoverage: [0, 1, 2, 3], // mirrors COVERAGE in enrich-roads-mx.ts (R1 auditable)
  },
  {
    id: 9012, // DZ = 12
    layer: 'roads',
    key: 'dz-national-roads',
    name: 'Algeria-tuned CNOSSOS class defaults (no open per-segment AADT)',
    year: 2024,
    license: 'derived-from-OSM',
    url: null,
    priority: 80,
    measurement: 'proxy',
    roadCoverage: [0, 1, 2, 3, 4, 5, 10, 11, 12], // mirrors DZ_COVERAGE in enrich-roads-dz.ts (R1b bucket A)
  },
  {
    id: 9364, // IR = 364
    layer: 'roads',
    key: 'ir-national-roads',
    name: 'Iran-tuned CNOSSOS class defaults (no open per-segment AADT)',
    year: 2024,
    license: 'derived-from-OSM',
    url: null,
    priority: 80,
    measurement: 'proxy',
    roadCoverage: [0, 1, 2, 3, 4, 5, 10, 11, 12], // mirrors IR_COVERAGE in enrich-roads-ir.ts (R1b bucket A)
  },
  {
    id: 9404, // KE = 404
    layer: 'roads',
    key: 'ke-national-roads',
    name: 'Kenya-tuned CNOSSOS class defaults (no open per-segment AADT)',
    year: 2024,
    license: 'derived-from-OSM',
    url: null,
    priority: 80,
    measurement: 'proxy',
    roadCoverage: [0, 1, 2, 3, 4, 5, 10, 11, 12], // mirrors KE_COVERAGE in enrich-roads-ke.ts (R1b bucket A)
  },
  {
    id: 9792, // TR = 792
    layer: 'roads',
    key: 'tr-national-roads',
    name: 'Turkey-tuned CNOSSOS class defaults (KGM otoyol/devlet yolu context)',
    year: 2024,
    license: 'derived-from-OSM',
    url: null,
    priority: 80,
    measurement: 'proxy',
    roadCoverage: [0, 1, 2, 3, 4, 5, 10, 11, 12], // mirrors TR_COVERAGE in enrich-roads-tr.ts (R1b bucket A)
  },
  {
    id: 9804, // UA = 804
    layer: 'roads',
    key: 'ua-national-roads',
    name: 'Ukraine-tuned CNOSSOS class defaults (Ukravtodor context)',
    year: 2024,
    license: 'derived-from-OSM',
    url: null,
    priority: 80,
    measurement: 'proxy',
    roadCoverage: [0, 1, 2, 3, 4, 5, 10, 11, 12], // mirrors UA_COVERAGE in enrich-roads-ua.ts (R1b bucket A)
  },
  {
    id: 9231, // ET = 231
    layer: 'roads',
    key: 'et-national-roads',
    name: 'Ethiopia-tuned CNOSSOS class defaults (ERA context; no open per-segment AADT)',
    year: 2024,
    license: 'derived-from-OSM',
    url: null,
    priority: 80,
    measurement: 'proxy',
    roadCoverage: [0, 1, 2, 3, 4, 5, 10, 11, 12], // mirrors ET_COVERAGE in enrich-roads-et.ts (R1b bucket A)
  },
  {
    id: 9180, // CD = 180
    layer: 'roads',
    key: 'cd-national-roads',
    name: 'DR Congo-tuned CNOSSOS class defaults (no open per-segment AADT)',
    year: 2024,
    license: 'derived-from-OSM',
    url: null,
    priority: 80,
    measurement: 'proxy',
    roadCoverage: [0, 1, 2, 3, 4, 5, 10, 11, 12], // mirrors CD_COVERAGE in enrich-roads-cd.ts (R1b bucket A)
  },
  {
    id: 9834, // TZ = 834
    layer: 'roads',
    key: 'tz-national-roads',
    name: 'Tanzania-tuned CNOSSOS class defaults (TANROADS context)',
    year: 2024,
    license: 'derived-from-OSM',
    url: null,
    priority: 80,
    measurement: 'proxy',
    roadCoverage: [0, 1, 2, 3, 4, 5, 10, 11, 12], // mirrors TZ_COVERAGE in enrich-roads-tz.ts (R1b bucket A)
  },
  {
    id: 9368, // IQ = 368
    layer: 'roads',
    key: 'iq-national-roads',
    name: 'Iraq-tuned CNOSSOS class defaults (no open per-segment AADT)',
    year: 2024,
    license: 'derived-from-OSM',
    url: null,
    priority: 80,
    measurement: 'proxy',
    roadCoverage: [0, 1, 2, 3, 4, 5, 10, 11, 12], // mirrors IQ_COVERAGE in enrich-roads-iq.ts (R1b bucket A)
  },
  {
    id: 9729, // SD = 729
    layer: 'roads',
    key: 'sd-national-roads',
    name: 'Sudan-tuned CNOSSOS class defaults (no open per-segment AADT)',
    year: 2024,
    license: 'derived-from-OSM',
    url: null,
    priority: 80,
    measurement: 'proxy',
    roadCoverage: [0, 1, 2, 3, 4, 5, 10, 11, 12], // mirrors SD_COVERAGE in enrich-roads-sd.ts (R1b bucket A)
  },
  {
    id: 9504, // MA = 504
    layer: 'roads',
    key: 'ma-national-roads',
    name: 'Morocco-tuned CNOSSOS class defaults (no open per-segment AADT)',
    year: 2024,
    license: 'derived-from-OSM',
    url: null,
    priority: 80,
    measurement: 'proxy',
    roadCoverage: [0, 1, 2, 3, 4, 5, 10, 11, 12], // mirrors MA_COVERAGE in enrich-roads-ma.ts (R1b bucket A)
  },
  {
    id: 9860, // UZ = 860
    layer: 'roads',
    key: 'uz-national-roads',
    name: 'Uzbekistan-tuned CNOSSOS class defaults (no open per-segment AADT)',
    year: 2024,
    license: 'derived-from-OSM',
    url: null,
    priority: 80,
    measurement: 'proxy',
    roadCoverage: [0, 1, 2, 3, 4, 5, 10, 11, 12], // mirrors UZ_COVERAGE in enrich-roads-uz.ts (R1b bucket A)
  },
  {
    id: 9398, // KZ = 398
    layer: 'roads',
    key: 'kz-national-roads',
    name: 'Kazakhstan-tuned CNOSSOS class defaults (no open per-segment AADT)',
    year: 2024,
    license: 'derived-from-OSM',
    url: null,
    priority: 80,
    measurement: 'proxy',
    roadCoverage: [0, 1, 2, 3, 4, 5, 10, 11, 12], // mirrors KZ_COVERAGE in enrich-roads-kz.ts (R1b bucket A)
  },

  // ── Railways: national (OSM-only) ──
  {
    id: 2000,
    layer: 'railways',
    key: 'ae-national-railway',
    name: 'Dubai RTA unified GTFS (via Dubai Pulse)',
    year: 2025,
    license: 'open-data',
    url: 'https://www.dubaipulse.gov.ae/',
    priority: 80,
    railFamilies: ['rail', 'tram'], // mirrors the match closure (/gg W5)
  },
  {
    id: 2004,
    layer: 'railways',
    key: 'ar-national-railway',
    measurement: 'proxy',
    name: 'IGN GeoServer (Trenes Argentinos + Subte BA)',
    year: 2024,
    license: 'public-data',
    url: 'https://wms.ign.gob.ar/geoserver/transporte/ows',
    priority: 80,
  },
  {
    id: 2005,
    layer: 'railways',
    key: 'au-national-railway',
    name: 'Australian state rail GTFS (TfNSW + Transperth + Adelaide Metro)',
    year: 2025,
    license: 'CC-BY-4.0',
    url: 'https://opendata.transport.nsw.gov.au/',
    priority: 80,
    railFamilies: ['rail', 'tram'],
  },
  {
    id: 2009,
    layer: 'railways',
    key: 'be-national-railway',
    name: 'Belgian urban rail GTFS (STIB/MIVB + De Lijn + TEC)',
    year: 2025,
    license: 'open-data',
    url: 'https://stibmivb.opendatasoft.com/',
    priority: 80,
    railFamilies: ['tram'], // match claims tram/light_rail only (/gg W5),
  },
  {
    id: 2015,
    layer: 'railways',
    key: 'ca-national-railway',
    name: 'Canadian rail GTFS (VIA Rail + Metrolinx + TTC + STM + TransLink + OC Transpo + Calgary + ETS)',
    year: 2025,
    license: 'open-data',
    url: 'https://www.viarail.ca/',
    priority: 80,
    railFamilies: ['rail', 'tram'],
  },
  {
    id: 2021,
    layer: 'railways',
    key: 'cn-national-railway',
    measurement: 'proxy',
    name: 'Mainland CR + Metros (ArcGIS FeatureServer mirror)',
    year: 2024,
    license: 'community-mirror',
    url: 'https://services7.arcgis.com/m6uLpqj7MgjPU371/',
    priority: 80,
    railFamilies: ['rail', 'tram'], // mirrors the match closure (/gg W5)
  },
  {
    // gtfs.de flattens the DELFI NAP NeTEx dataset (all ~12 state systems +
    // DB long-distance) into one plain-GTFS national aggregate. Passenger-only:
    // DB InfraGO publishes no freight paths (rail-timetable acquisition matrix
    // 2026-07), so enrich-railway-de.ts stamps frt=0 and the engine's per-column
    // zero-default supplies interim freight (normalize/rail.rs). id 9864 =
    // allocator max+1; the 2xxx rail block was a one-time batch, not reserved slots.
    id: 9864,
    layer: 'railways',
    key: 'de-national-railway',
    name: 'DELFI national GTFS via gtfs.de (de_full)',
    year: 2026,
    license: 'CC-BY-4.0',
    url: 'https://gtfs.de/en/feeds/de_full/',
    priority: 80,
    measurement: 'counted',
    railFamilies: ['rail', 'tram'],
  },
  {
    id: 2024,
    layer: 'railways',
    key: 'dk-national-railway',
    name: 'Rejseplanen unified GTFS',
    year: 2025,
    license: 'open-data',
    url: 'https://www.rejseplanen.info/labs/GTFS.zip',
    priority: 80,
    railFamilies: ['rail', 'tram'],
  },
  {
    id: 2028,
    layer: 'railways',
    key: 'es-national-railway',
    name: 'Renfe + FGC GTFS',
    year: 2025,
    license: 'CC-BY-4.0',
    url: 'https://data.renfe.com/',
    priority: 80,
    railFamilies: ['rail'], // Renfe/FGC heavy/suburban only; street trams are separate operators
  },
  {
    id: 2030,
    layer: 'railways',
    key: 'fi-national-railway',
    name: 'Finnish rail GTFS (Fintraffic VR + HSL + Tampere Raitiotie)',
    year: 2025,
    license: 'CC-BY-4.0',
    url: 'https://rata.digitraffic.fi/',
    priority: 80,
    railFamilies: ['rail', 'tram'],
  },
  {
    id: 2035,
    layer: 'railways',
    key: 'ie-national-railway',
    name: 'NTA Transport for Ireland unified GTFS',
    year: 2025,
    license: 'open-data',
    url: 'https://www.transportforireland.ie/',
    priority: 80,
    railFamilies: ['rail', 'tram'],
  },
  {
    id: 2036,
    layer: 'railways',
    key: 'il-national-railway',
    name: 'Israeli MoT unified GTFS',
    year: 2025,
    license: 'CC-BY-4.0',
    url: 'https://www.gov.il/he/pages/gtfs_general_transit_feed_specifications',
    priority: 80,
    railFamilies: ['rail', 'tram'],
  },
  {
    id: 2037,
    layer: 'railways',
    key: 'in-national-railway',
    measurement: 'proxy',
    name: 'Living Atlas IN Railway Network',
    year: 2024,
    license: 'public-data',
    url: 'https://livingatlas.esri.in/',
    priority: 80,
    railFamilies: ['rail', 'tram'], // mirrors the match closure (/gg W5)
  },
  {
    id: 2040,
    layer: 'railways',
    key: 'it-national-railway',
    name: 'Italian regional rail GTFS (Trenitalia + Trenord + GTT + Ferrotramviaria)',
    year: 2025,
    license: 'CC-BY-4.0',
    url: 'https://dati.toscana.it/',
    priority: 80,
    railFamilies: ['rail', 'tram'],
  },
  {
    id: 2044,
    layer: 'railways',
    key: 'kr-national-railway',
    measurement: 'proxy',
    name: 'KORAIL operator-class CNOSSOS defaults',
    year: 2025,
    license: 'derived-from-OSM',
    url: null,
    priority: 80,
  },
  {
    id: 2057,
    layer: 'railways',
    key: 'mx-national-railway',
    name: 'CDMX SEMOVI unified GTFS',
    year: 2025,
    license: 'open-data',
    url: 'https://datos.cdmx.gob.mx/dataset/gtfs',
    priority: 80,
    railFamilies: ['rail', 'tram'],
  },
  {
    id: 2066,
    layer: 'railways',
    key: 'pl-national-railway',
    name: 'Polish Trains unified GTFS (Kuranowski/PKP PLK)',
    year: 2025,
    license: 'CC-BY-4.0',
    url: 'https://mkuran.pl/gtfs/',
    priority: 80,
    railFamilies: ['rail', 'tram'],
  },
  {
    id: 2067,
    layer: 'railways',
    key: 'pt-national-railway',
    name: 'Portuguese rail GTFS (CP + Metro do Porto + MTS)',
    year: 2025,
    license: 'open-data',
    url: 'https://publico.cp.pt/gtfs',
    priority: 80,
    railFamilies: ['rail', 'tram'],
  },
  {
    id: 2073,
    layer: 'railways',
    key: 'se-national-railway',
    name: 'GTFS Sverige 2 unified (Trafiklab/Samtrafiken)',
    year: 2025,
    license: 'open-data',
    url: 'https://www.trafiklab.se/',
    priority: 80,
    railFamilies: ['rail', 'tram'],
  },
  {
    id: 2075,
    layer: 'railways',
    key: 'th-national-railway',
    name: 'Namtang GTFS (Thailand)',
    year: 2025,
    license: 'open-data',
    url: null,
    priority: 80,
    railFamilies: ['rail', 'tram'],
  },
  {
    // 9000 + ISO-3166 numeric (RU = 643) + 1 — roads took 9643, railway takes 9644.
    id: 9644,
    layer: 'railways',
    key: 'ru-national-railway',
    measurement: 'proxy',
    name: 'Russia operator-class CNOSSOS defaults (no open per-segment timetable)',
    year: 2024,
    license: 'derived-from-OSM',
    url: null,
    priority: 80,
    railFamilies: ['rail'], // class defaults by family; no GTFS-measured tram counts
  },
  {
    id: 9567, // NG = 566 (+1 railway)
    layer: 'railways',
    key: 'ng-national-railway',
    measurement: 'proxy',
    name: 'Nigeria operator-class CNOSSOS defaults (SGR commissioning context)',
    year: 2024,
    license: 'derived-from-OSM',
    url: null,
    priority: 80,
    railFamilies: ['rail'],
  },
  {
    id: 9819, // EG = 818 (+1 railway)
    layer: 'railways',
    key: 'eg-national-railway',
    measurement: 'proxy',
    name: 'Egypt operator-class CNOSSOS defaults (no open per-segment timetable)',
    year: 2024,
    license: 'derived-from-OSM',
    url: null,
    priority: 80,
    railFamilies: ['rail'],
  },
  {
    id: 9013, // DZ = 12 (+1 railway)
    layer: 'railways',
    key: 'dz-national-railway',
    measurement: 'proxy',
    name: 'Algeria operator-class CNOSSOS defaults (SNTF context)',
    year: 2024,
    license: 'derived-from-OSM',
    url: null,
    priority: 80,
    railFamilies: ['rail'],
  },
  {
    id: 9365, // IR = 364 (+1 railway)
    layer: 'railways',
    key: 'ir-national-railway',
    measurement: 'proxy',
    name: 'Iran operator-class CNOSSOS defaults (RAI context)',
    year: 2024,
    license: 'derived-from-OSM',
    url: null,
    priority: 80,
    railFamilies: ['rail'],
  },
  {
    id: 9405, // KE = 404 (+1 railway)
    layer: 'railways',
    key: 'ke-national-railway',
    measurement: 'proxy',
    name: 'Kenya operator-class CNOSSOS defaults (SGR/metre-gauge context)',
    year: 2024,
    license: 'derived-from-OSM',
    url: null,
    priority: 80,
    railFamilies: ['rail'],
  },
  {
    id: 9793, // TR = 792 (+1 railway)
    layer: 'railways',
    key: 'tr-national-railway',
    measurement: 'proxy',
    name: 'Turkey operator-class CNOSSOS defaults (TCDD context)',
    year: 2024,
    license: 'derived-from-OSM',
    url: null,
    priority: 80,
    railFamilies: ['rail'],
  },
  {
    id: 9805, // UA = 804 (+1 railway)
    layer: 'railways',
    key: 'ua-national-railway',
    measurement: 'proxy',
    name: 'Ukraine operator-class CNOSSOS defaults (Ukrzaliznytsia context)',
    year: 2024,
    license: 'derived-from-OSM',
    url: null,
    priority: 80,
    railFamilies: ['rail'],
  },
  {
    id: 9232, // ET = 231 (+1 railway)
    layer: 'railways',
    key: 'et-national-railway',
    measurement: 'proxy',
    name: 'Ethiopia operator-class CNOSSOS defaults (Addis-Djibouti SGR + LRT context)',
    year: 2024,
    license: 'derived-from-OSM',
    url: null,
    priority: 80,
    railFamilies: ['rail'],
  },
  {
    id: 9181, // CD = 180 (+1 railway)
    layer: 'railways',
    key: 'cd-national-railway',
    measurement: 'proxy',
    name: 'DR Congo operator-class CNOSSOS defaults (SNCC context)',
    year: 2024,
    license: 'derived-from-OSM',
    url: null,
    priority: 80,
    railFamilies: ['rail'],
  },
  {
    id: 9835, // TZ = 834 (+1 railway)
    layer: 'railways',
    key: 'tz-national-railway',
    measurement: 'proxy',
    name: 'Tanzania operator-class CNOSSOS defaults (TRC SGR + TAZARA context)',
    year: 2024,
    license: 'derived-from-OSM',
    url: null,
    priority: 80,
    railFamilies: ['rail'],
  },
  {
    id: 9369, // IQ = 368 (+1 railway)
    layer: 'railways',
    key: 'iq-national-railway',
    measurement: 'proxy',
    name: 'Iraq operator-class CNOSSOS defaults (IRR context)',
    year: 2024,
    license: 'derived-from-OSM',
    url: null,
    priority: 80,
    railFamilies: ['rail'],
  },
  {
    id: 9730, // SD = 729 (+1 railway)
    layer: 'railways',
    key: 'sd-national-railway',
    measurement: 'proxy',
    name: 'Sudan operator-class CNOSSOS defaults (SRC context)',
    year: 2024,
    license: 'derived-from-OSM',
    url: null,
    priority: 80,
    railFamilies: ['rail'],
  },
  {
    id: 9505, // MA = 504 (+1 railway)
    layer: 'railways',
    key: 'ma-national-railway',
    measurement: 'proxy',
    name: 'Morocco operator-class CNOSSOS defaults (ONCF Al Boraq HSR + conventional context)',
    year: 2024,
    license: 'derived-from-OSM',
    url: null,
    priority: 80,
    railFamilies: ['rail'],
  },
  {
    id: 9861, // UZ = 860 (+1 railway)
    layer: 'railways',
    key: 'uz-national-railway',
    measurement: 'proxy',
    name: 'Uzbekistan operator-class CNOSSOS defaults (UTY Afrosiyob HSR + conventional context)',
    year: 2024,
    license: 'derived-from-OSM',
    url: null,
    priority: 80,
    railFamilies: ['rail'],
  },
  {
    id: 9399, // KZ = 398 (+1 railway)
    layer: 'railways',
    key: 'kz-national-railway',
    measurement: 'proxy',
    name: 'Kazakhstan operator-class CNOSSOS defaults (KTZ context)',
    year: 2024,
    license: 'derived-from-OSM',
    url: null,
    priority: 80,
    railFamilies: ['rail'],
  },

  // ── Heuristics ──
  {
    id: 9000,
    layer: 'industrial',
    key: 'industrial-name-heuristic',
    name: 'OSM industrial name-keyword NACE heuristic',
    year: 2025,
    license: 'project-internal',
    url: null,
    priority: 10,
  },
  {
    // KR = 410. priority 10 (heuristic, same tier as 9000) but id > 9000 so the
    // Korean-script keyword match wins the equal-rank id-tiebreak over the
    // Latin-only global heuristic it supersedes; still < GEM/GPPD (priority 50),
    // which keep their measured matches. See enrich-industrial-kr.ts docstring.
    id: 9410,
    layer: 'industrial',
    key: 'kr-industrial-names',
    name: 'OSM Korean-script industrial name-keyword NACE heuristic',
    year: 2025,
    license: 'project-internal',
    url: null,
    priority: 10,
  },

  // ── Global raster baselines ──
  // Not per-row arrow provenance (rasters have no row ids); registered here
  // for completeness so docs/UI can reference the source by name.
  {
    id: 9001,
    layer: 'buildings',
    key: 'global-overture',
    name: 'Overture Maps buildings (MS/Google/Meta)',
    year: 2024,
    license: 'CDLA-Permissive-2.0',
    url: 'https://overturemaps.org/',
    priority: 50,
    provenance: 'baseline', // OSM-derived inference, not a measurement
  },
  {
    id: 9002,
    layer: 'any',
    key: 'global-copernicus-glo30',
    name: 'Copernicus GLO-30 DEM',
    year: 2021,
    license: 'CC-BY-4.0',
    url: 'https://spacedata.copernicus.eu/collections/copernicus-digital-elevation-model',
    priority: 50,
    provenance: 'baseline', // global raster baseline, not a measurement
  },
  // Structure-table height ladder (scripts/structures/build-structures.py):
  // structure rows carry `height_tier`, not `source_id` — these entries
  // document the rasters' license/URL/rank for docs + attribution (tier 4 =
  // ANBH, tier 3 = city DSM).
  {
    id: 9866,
    layer: 'buildings',
    key: 'global-ghsl-built-h',
    name: 'GHS-BUILT-H R2023A ANBH building heights (JRC)',
    year: 2018,
    license: 'CC-BY-4.0',
    url: 'https://human-settlement.emergency.copernicus.eu/ghs_buH2023.php',
    priority: 50,
    provenance: 'baseline', // 100 m areal average, not a per-building measurement
  },
  {
    id: 9867,
    layer: 'buildings',
    key: 'cz-ipr-praha-vysky',
    name: 'IPR Praha – Relativní výšky budov (building DSM−DTM, 1 m)',
    year: 2023, // dataset revision per IPR metadata; 2026 is only the metadata-update date
    license: 'CC-BY-4.0',
    url: 'https://opendata.geoportalpraha.cz/maps/ad9aca20e9c042d2b52eb31ff18961b6',
    priority: 90, // city-measured
  },
]

// `DATASETS_BY_ID` / `DATASETS_BY_KEY` / `UNSPECIFIED` were the legacy
// fast-lookup exports; everything has now migrated to the
// provenance-aware `SOURCES_BY_*` registry in `pipeline/lib/sources.ts`.
// `DATASETS` and `Dataset` remain the single source of truth for the raw
// rows the new registry layers on top of.
