//! Survivability map: every column of every per-hex arrow layer → its parent
//! PRODUCERS as structured tokens (below). `--inventory` walks the reference
//! hex's files, joins them against this map AND validates every producer token
//! against the resolved world-scope manifest plan — a live column with no
//! entry here is an ORPHAN (data nothing would recreate after a fresh extract
//! + chain run), and a token naming a nonexistent step id fails the inventory
//! (no substring matching — the pre-2026-07-11 checks were tautologies that a
//! renamed step could never trip).
//!
//! Producer token grammar (resolved by chain/inventory.ts):
//!   'extract:<file.rs>'           osm-extract finalize writer (not a chain step)
//!   'aircraft-extract:<path.rs>'  aircraft-lane writer (scripts/run-extraction.sh)
//!   'overture-parquet:<path.py>'  external Overture one-degree parquet cache fill
//!                                 (downloaded, never a chain step)
//!   'national:<layer>'            every national-phase manifest step of that layer
//!   'city:<layer>'                every city-phase manifest step of that layer
//!   anything else                 EXACT manifest step id (e.g. 'roads-built-up')
//!
//! Provenance of the extractor facts: engine/osm-extract/src/finalize/write_*.rs
//! and engine/aircraft-extract/src/arrow_schemas.rs (+ arrow_io/*.rs,
//! synth_airport_io.rs), read 2026-07-11. CHAIN-ONLY columns (chainOnly: true)
//! do not exist in a fresh extract at all — the enrichment writers materialize
//! them (writeRoadAadt / writeRailTrains / writeRailParallelDivisor /
//! enrich-roads-built-up / enrich-industrial-gem); that is exactly the set the
//! R8 chain exists to restore.

export interface ColumnOrigin {
  /** Producer tokens (grammar in the module doc) — every token must resolve
   *  against the world-scope manifest plan or the inventory fails. */
  parents: readonly string[]
  /** True when a fresh OSM extract does NOT recreate the column — only the chain does. */
  chainOnly?: boolean
  /** Human nuance that must not water down the structured tokens (WHY/provenance). */
  note?: string
}

const EXTRACT_ROADS: ColumnOrigin = { parents: ['extract:write_roads.rs'] }
const EXTRACT_RAILWAYS: ColumnOrigin = { parents: ['extract:write_railways.rs'] }
const EXTRACT_INDUSTRIAL: ColumnOrigin = { parents: ['extract:write_industrial.rs'] }
const EXTRACT_LEISURE: ColumnOrigin = { parents: ['extract:write_leisure.rs'] }
const EXTRACT_AIRPORT: ColumnOrigin = { parents: ['extract:write_airport.rs'] }
const AIRBORNE: ColumnOrigin = { parents: ['aircraft-extract:arrow_io/airborne.rs'] }
const CRUISE: ColumnOrigin = { parents: ['aircraft-extract:arrow_io/cruise.rs'] }
const AIRPORT_TRAFFIC: ColumnOrigin = { parents: ['aircraft-extract:arrow_io/airport_traffic.rs'] }
const SYNTH_AIRPORT: ColumnOrigin = { parents: ['aircraft-extract:synth_airport_io.rs'] }
const OVERTURE_PARQUETS = 'overture-parquet:scripts/obstacles/download-overture-tiles.py'

/** The ONE per-cell structure table's merge step (scripts/structures/build-structures.py,
 *  wrapped by the enrich-structures.ts face): consumes the pre-merge osm-extract files
 *  (buildings.arrow after its enrichers, barriers.arrow) + the Overture parquet cache +
 *  the GHSL/regional height rasters, writes structures.arrow. */
const STRUCTURES = 'structures'

/** All four AADT columns share one producer set: writeRoadAadt, called by the
 *  national census family, the city drivers and the three heuristic passes. */
const AADT: ColumnOrigin = {
  parents: ['national:roads', 'city:roads', 'roads-service-tree', 'roads-continuity-fill', 'roads-taper'],
  chainOnly: true,
  note: 'writeRoadAadt (lib/roads-arrow.ts) — the ONE road payload writer',
}

const TRAINS: ColumnOrigin = {
  parents: ['national:railways', 'railway-europe'],
  chainOnly: true,
  note: 'writeRailTrains (lib/railways-arrow.ts) — the ONE rail payload writer',
}

/** Per layer file: column → origin. Nested struct children are covered by their
 *  top-level column (sub_segments, top_candidates). */
export const COLUMN_PARENTS: Readonly<Record<string, Readonly<Record<string, ColumnOrigin>>>> = {
  'roads.arrow': {
    osm_id: EXTRACT_ROADS,
    segment_idx: EXTRACT_ROADS,
    start_lat: EXTRACT_ROADS,
    start_lon: EXTRACT_ROADS,
    end_lat: EXTRACT_ROADS,
    end_lon: EXTRACT_ROADS,
    length_m: EXTRACT_ROADS,
    road_class: EXTRACT_ROADS,
    speed_limit: { parents: ['extract:write_roads.rs'], note: 'OSM maxspeed tag; NEVER written by any enricher' },
    surface_type: EXTRACT_ROADS,
    oneway: EXTRACT_ROADS,
    lanes: EXTRACT_ROADS,
    name: EXTRACT_ROADS,
    ref: EXTRACT_ROADS,
    bridge: EXTRACT_ROADS,
    tunnel: EXTRACT_ROADS,
    toll: EXTRACT_ROADS,
    lit: EXTRACT_ROADS,
    junction: EXTRACT_ROADS,
    access: EXTRACT_ROADS,
    built_up: {
      parents: ['roads-built-up'],
      chainOnly: true,
      note: 'u8 rural/urban; also run by the osm-to-h3r4.sh tail (byte-identical re-run)',
    },
    speed_taper: {
      parents: ['roads-taper'],
      chainOnly: true,
      note: 'writeRoadAadt speed_taper channel; taper-owned, cleared on overwrite/retract',
    },
    country_iso: {
      parents: ['roads-country'],
      chainOnly: true,
      note: 'M3 bake: u16 iso0|iso1<<8, 0=\\0\\0; all-or-none with city_id/continent + roads_contract=country_baked_v1',
    },
    city_id: {
      parents: ['roads-country'],
      chainOnly: true,
      note: 'M3 bake: u16 metro id (h3-admin-metros.json), 0=none',
    },
    continent: {
      parents: ['roads-country'],
      chainOnly: true,
      note: 'M3 bake: u8 engine Continent id (admin.rs repr(u8))',
    },
    aadt_light: AADT,
    aadt_medium: AADT,
    aadt_heavy: AADT,
    aadt_moto: AADT,
    source_id: { parents: ['extract:write_roads.rs'], note: 'constant 0 at extract → stamped by every roads step via writeRoadAadt' },
  },
  'railways.arrow': {
    osm_id: EXTRACT_RAILWAYS,
    segment_idx: EXTRACT_RAILWAYS,
    start_lat: EXTRACT_RAILWAYS,
    start_lon: EXTRACT_RAILWAYS,
    end_lat: EXTRACT_RAILWAYS,
    end_lon: EXTRACT_RAILWAYS,
    length_m: EXTRACT_RAILWAYS,
    rail_type: EXTRACT_RAILWAYS,
    usage: EXTRACT_RAILWAYS,
    maxspeed: EXTRACT_RAILWAYS,
    name: EXTRACT_RAILWAYS,
    ref: EXTRACT_RAILWAYS,
    electrified: EXTRACT_RAILWAYS,
    gauge: EXTRACT_RAILWAYS,
    bridge: EXTRACT_RAILWAYS,
    tunnel: EXTRACT_RAILWAYS,
    highspeed: EXTRACT_RAILWAYS,
    service: EXTRACT_RAILWAYS,
    trains_passenger: TRAINS,
    trains_freight: TRAINS,
    parallel_divisor: {
      parents: ['railway-cz', 'railways-parallel'],
      chainOnly: true,
      note: 'railway-cz czpttKey identity divisors + railways-parallel only-raise shared pass (writeRailParallelDivisor)',
    },
    country_iso: {
      parents: ['roads-country'],
      chainOnly: true,
      note: 'M3 bake: u16 iso0|iso1<<8, 0=\\0\\0; all-or-none with city_id/continent + railways_contract=country_baked_v1',
    },
    city_id: {
      parents: ['roads-country'],
      chainOnly: true,
      note: 'M3 bake: u16 metro id (h3-admin-metros.json), 0=none',
    },
    continent: {
      parents: ['roads-country'],
      chainOnly: true,
      note: 'M3 bake: u8 engine Continent id (admin.rs repr(u8))',
    },
    source_id: { parents: ['extract:write_railways.rs'], note: 'constant 0 at extract → stamped by every railway step via writeRailTrains' },
  },
  'industrial.arrow': {
    osm_id: EXTRACT_INDUSTRIAL,
    centroid_lat: EXTRACT_INDUSTRIAL,
    centroid_lon: EXTRACT_INDUSTRIAL,
    source_type: { parents: ['extract:write_industrial.rs', 'global-windturbines'], note: 'refined by industrial steps (wind=10)' },
    site_subtype: { parents: ['extract:write_industrial.rs', 'national:industrial'], note: 'patched by lib/enrich-industrial-gem.ts' },
    name: EXTRACT_INDUSTRIAL,
    hub_height: { parents: ['extract:write_industrial.rs', 'global-windturbines'] },
    rated_power_kw: { parents: ['extract:write_industrial.rs', 'global-windturbines', 'national:industrial'] },
    polygon_wkb: EXTRACT_INDUSTRIAL,
    area_m2: EXTRACT_INDUSTRIAL,
    nace_4digit: {
      parents: ['national:industrial', 'global-industrial', 'industrial-name-heuristic'],
      chainOnly: true,
      note: 'lib/enrich-industrial-gem.ts NACE sidecar column',
    },
    source_id: { parents: ['extract:write_industrial.rs'], note: 'constant 0 at extract → stamped by every industrial step' },
  },
  'structures.arrow': {
    kind: { parents: [STRUCTURES], note: '0 building / 1 barrier — written by the builder for every row' },
    geometry_wkb: {
      parents: [OVERTURE_PARQUETS, 'extract:write_buildings.rs', 'extract:write_barriers.rs', STRUCTURES],
      note: 'screening polygon: Overture WKB on matched/Overture-only rows (a matched pair keeps the Overture geometry — census: 0 of 2.83 M matched pairs share WKB), OSM polygon on OSM-only rows; barrier rows carry the builder-encoded 2-point wall WKB',
    },
    height_m: {
      parents: [OVERTURE_PARQUETS, 'extract:write_buildings.rs', 'extract:write_barriers.rs', STRUCTURES],
      note: 'the builder ladder: mapped → floors×3 → regional zonal (tier 3) / GHSL ANBH (tier 4) → 8 m default; walls carry the barriers.arrow spill-resolved height',
    },
    height_tier: {
      parents: [OVERTURE_PARQUETS, 'extract:write_buildings.rs', STRUCTURES],
      note: 'ladder tier 0-4 (noise_compute::low_profile); wall tiers are value-inferred for the 2026 world',
    },
    envelope_class: {
      parents: [OVERTURE_PARQUETS, 'extract:write_buildings.rs', STRUCTURES],
      note: 'Overture class/subtype map; OSM-only rows map building_use; walls are outdoor',
    },
    centroid_lat: {
      parents: [OVERTURE_PARQUETS, 'extract:write_buildings.rs', 'extract:write_barriers.rs', STRUCTURES],
      note: 'screening centroid: the Overture centroid on matched rows; walls take the segment midpoint',
    },
    centroid_lon: {
      parents: [OVERTURE_PARQUETS, 'extract:write_buildings.rs', 'extract:write_barriers.rs', STRUCTURES],
      note: 'screening centroid: the Overture centroid on matched rows; walls take the segment midpoint',
    },
    osm_id: {
      parents: ['extract:write_buildings.rs', 'extract:write_barriers.rs', STRUCTURES],
      note: 'OSM building id on attributed building rows, wall id on barrier rows (ScreeningSourceId); null on Overture-only rows',
    },
    building_type: { parents: ['extract:write_buildings.rs', 'buildings-cz', STRUCTURES], note: 'POI-join v2 at extract; RÚIAN refines the pre-merge buildings.arrow' },
    building_use: { parents: ['extract:write_buildings.rs', STRUCTURES] },
    height: { parents: ['extract:write_buildings.rs', STRUCTURES], note: 'raw OSM height tag (emission input)' },
    floors: { parents: ['extract:write_buildings.rs', 'buildings-cz', 'buildings-es', STRUCTURES], note: 'writeBuildingEnrichment refines pre-merge' },
    name: { parents: ['extract:write_buildings.rs', STRUCTURES] },
    addr_street: { parents: ['extract:write_buildings.rs', STRUCTURES] },
    addr_housenumber: { parents: ['extract:write_buildings.rs', STRUCTURES] },
    area_m2: { parents: ['extract:write_buildings.rs', STRUCTURES] },
    opening_hours_frac: { parents: ['extract:write_buildings.rs', STRUCTURES] },
    source_id: { parents: ['extract:write_buildings.rs', STRUCTURES], note: 'constant 0 at extract → stamped by buildings-cz/-es pre-merge' },
    emission_polygon_wkb: {
      parents: [STRUCTURES],
      note: 'sparse emission override: the OSM polygon where emission can read it (area missing or > 2000 m2) and it differs from the screening polygon; null → geometry_wkb',
    },
    emission_centroid_lat: {
      parents: [STRUCTURES],
      note: 'the OSM centroid on matched rows (centroid_* is the Overture one there, read by the low-profile cap); null → centroid_*',
    },
    emission_centroid_lon: {
      parents: [STRUCTURES],
      note: 'the OSM centroid on matched rows (centroid_* is the Overture one there, read by the low-profile cap); null → centroid_*',
    },
    segment_idx: { parents: ['extract:write_barriers.rs', STRUCTURES], note: 'barrier rows only (ScreeningSourceId data)' },
  },
  'leisure.arrow': {
    osm_id: EXTRACT_LEISURE,
    centroid_lat: EXTRACT_LEISURE,
    centroid_lon: EXTRACT_LEISURE,
    sport: EXTRACT_LEISURE,
    capacity: { parents: ['extract:write_leisure.rs'], note: 'vestigial, unread downstream' },
    opening_hours_frac: EXTRACT_LEISURE,
    name: EXTRACT_LEISURE,
    polygon_wkb: EXTRACT_LEISURE,
    area_m2: EXTRACT_LEISURE,
  },
  'airport_areas.arrow': {
    osm_id: EXTRACT_AIRPORT,
    centroid_lat: EXTRACT_AIRPORT,
    centroid_lon: EXTRACT_AIRPORT,
    aeroway_type: EXTRACT_AIRPORT,
    name: EXTRACT_AIRPORT,
    ref: EXTRACT_AIRPORT,
    icao: EXTRACT_AIRPORT,
    iata: EXTRACT_AIRPORT,
    operator: EXTRACT_AIRPORT,
    surface: EXTRACT_AIRPORT,
    width_m: EXTRACT_AIRPORT,
    aerodrome_type: EXTRACT_AIRPORT,
    access: EXTRACT_AIRPORT,
    polygon_wkb: EXTRACT_AIRPORT,
    area_m2: EXTRACT_AIRPORT,
  },
  'airport_lines.arrow': {
    osm_id: EXTRACT_AIRPORT,
    segment_idx: EXTRACT_AIRPORT,
    start_lat: EXTRACT_AIRPORT,
    start_lon: EXTRACT_AIRPORT,
    end_lat: EXTRACT_AIRPORT,
    end_lon: EXTRACT_AIRPORT,
    length_m: EXTRACT_AIRPORT,
    heading_deg: EXTRACT_AIRPORT,
    aeroway_type: EXTRACT_AIRPORT,
    ref: EXTRACT_AIRPORT,
    surface: EXTRACT_AIRPORT,
    width_m: EXTRACT_AIRPORT,
  },
  // Aircraft-lane files: produced by engine/aircraft-extract stages, NOT by this
  // chain; scripts/run-extraction.sh runs them after the OSM extract. Listed so
  // the inventory can prove they are
  // parented — orphan detection must cover every file a hex dir carries.
  'airborne.arrow': {
    flight_id: AIRBORNE,
    callsign: AIRBORNE,
    aircraft_type: AIRBORNE,
    profile_idx: AIRBORNE,
    source_id: { parents: ['aircraft-extract:arrow_io/airborne.rs'], note: 'u8 ADS-B source code, NOT the enrichment provenance id' },
    origin: AIRBORNE,
    sub_segments: AIRBORNE,
    total_length_m: AIRBORNE,
    bbox_min_lat: AIRBORNE,
    bbox_max_lat: AIRBORNE,
    bbox_min_lon: AIRBORNE,
    bbox_max_lon: AIRBORNE,
  },
  'cruise.arrow': {
    r7_hex: CRUISE,
    class: CRUISE,
    rep_profile_idx: CRUISE,
    fl_bin: CRUISE,
    period: CRUISE,
    sum_length_m: CRUISE,
    rep_len_m: CRUISE,
    rep_alt_m: CRUISE,
    rep_speed_kt: CRUISE,
    unique_count: CRUISE,
    top_candidates: CRUISE,
    source_id: { parents: ['aircraft-extract:arrow_io/cruise.rs'], note: 'u8 ADS-B source code' },
    origin: CRUISE,
  },
  'airport_traffic.arrow': {
    airport_key: AIRPORT_TRAFFIC,
    osm_id: AIRPORT_TRAFFIC,
    segment_idx: AIRPORT_TRAFFIC,
    geometry_kind: AIRPORT_TRAFFIC,
    start_lat: AIRPORT_TRAFFIC,
    start_lon: AIRPORT_TRAFFIC,
    end_lat: AIRPORT_TRAFFIC,
    end_lon: AIRPORT_TRAFFIC,
    length_m: AIRPORT_TRAFFIC,
    ops_kind: AIRPORT_TRAFFIC,
    is_departure: AIRPORT_TRAFFIC,
    veh_kind: AIRPORT_TRAFFIC,
    class_idx: AIRPORT_TRAFFIC,
    period: AIRPORT_TRAFFIC,
    band_energy_lin: AIRPORT_TRAFFIC,
    unique_movement_count: AIRPORT_TRAFFIC,
    unique_arr_count: AIRPORT_TRAFFIC,
    unique_dep_count: AIRPORT_TRAFFIC,
    unique_gse_count_per_class: AIRPORT_TRAFFIC,
    microseg_unique_count: AIRPORT_TRAFFIC,
    microseg_unique_arr_count: AIRPORT_TRAFFIC,
    microseg_unique_dep_count: AIRPORT_TRAFFIC,
    microseg_unique_gse_count_per_class: AIRPORT_TRAFFIC,
    microseg_unique_ga_count: AIRPORT_TRAFFIC,
    microseg_unique_ga_arr_count: AIRPORT_TRAFFIC,
    microseg_unique_ga_dep_count: AIRPORT_TRAFFIC,
  },
  'synth_airport_areas.arrow': {
    osm_id: SYNTH_AIRPORT,
    airport_key: SYNTH_AIRPORT,
    name: SYNTH_AIRPORT,
    aeroway_type: SYNTH_AIRPORT,
    centroid_lat: SYNTH_AIRPORT,
    centroid_lon: SYNTH_AIRPORT,
    area_m2: SYNTH_AIRPORT,
  },
  'synth_airport_lines.arrow': {
    osm_id: SYNTH_AIRPORT,
    segment_idx: SYNTH_AIRPORT,
    airport_key: SYNTH_AIRPORT,
    start_lat: SYNTH_AIRPORT,
    start_lon: SYNTH_AIRPORT,
    end_lat: SYNTH_AIRPORT,
    end_lon: SYNTH_AIRPORT,
    length_m: SYNTH_AIRPORT,
    heading_deg: SYNTH_AIRPORT,
    aeroway_type: SYNTH_AIRPORT,
    name: SYNTH_AIRPORT,
  },
}

/** Pinned survivability assertions: the named producer token must be LISTED in
 *  the column's `parents` (exact membership, no substrings) AND resolve against
 *  the world-scope manifest plan — the tripwire against a refactor quietly
 *  orphaning a chain-owned column or renaming a step out from under the map. */
export const REQUIRED_PARENT_CHECKS: ReadonlyArray<{ file: string; column: string; parent: string }> = [
  { file: 'roads.arrow', column: 'built_up', parent: 'roads-built-up' },
  { file: 'roads.arrow', column: 'speed_taper', parent: 'roads-taper' },
  { file: 'roads.arrow', column: 'country_iso', parent: 'roads-country' },
  { file: 'roads.arrow', column: 'city_id', parent: 'roads-country' },
  { file: 'roads.arrow', column: 'continent', parent: 'roads-country' },
  { file: 'railways.arrow', column: 'country_iso', parent: 'roads-country' },
  { file: 'railways.arrow', column: 'city_id', parent: 'roads-country' },
  { file: 'railways.arrow', column: 'continent', parent: 'roads-country' },
  { file: 'railways.arrow', column: 'parallel_divisor', parent: 'railway-cz' },
  { file: 'railways.arrow', column: 'parallel_divisor', parent: 'railways-parallel' },
  { file: 'roads.arrow', column: 'aadt_light', parent: 'national:roads' },
  { file: 'roads.arrow', column: 'aadt_light', parent: 'roads-service-tree' },
  { file: 'roads.arrow', column: 'aadt_medium', parent: 'roads-continuity-fill' },
  { file: 'roads.arrow', column: 'aadt_heavy', parent: 'national:roads' },
  { file: 'roads.arrow', column: 'aadt_moto', parent: 'national:roads' },
  { file: 'railways.arrow', column: 'trains_passenger', parent: 'national:railways' },
  { file: 'railways.arrow', column: 'trains_passenger', parent: 'railway-europe' },
  { file: 'railways.arrow', column: 'trains_freight', parent: 'national:railways' },
  { file: 'industrial.arrow', column: 'nace_4digit', parent: 'national:industrial' },
  { file: 'industrial.arrow', column: 'nace_4digit', parent: 'global-industrial' },
  { file: 'structures.arrow', column: 'geometry_wkb', parent: OVERTURE_PARQUETS },
  { file: 'structures.arrow', column: 'osm_id', parent: 'extract:write_buildings.rs' },
  { file: 'structures.arrow', column: 'segment_idx', parent: 'extract:write_barriers.rs' },
  { file: 'structures.arrow', column: 'height_m', parent: 'structures' },
  { file: 'structures.arrow', column: 'height_tier', parent: 'structures' },
  { file: 'structures.arrow', column: 'emission_centroid_lat', parent: 'structures' },
]
