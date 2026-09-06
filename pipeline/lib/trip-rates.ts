/**
 * Daily vehicle-trip generation rates per building_type (0–13) — the SSOT the
 * service-tree road heuristic multiplies against building geometry.
 *
 * Unit: vehicle trips per day, both directions, annual average. Residential
 * arms return DWELLINGS instead (the caller multiplies by the per-country
 * `vehicle_trips_per_occupied_dwelling` from country-fleet.ts once
 * per segment); every other arm returns trips directly, so a country's
 * residential trip habit never rescales malls or factories.
 *
 * Non-residential rates trace to ITE Trip Generation Manual 11th Ed daily
 * rates. The CHANGED/NEW arms (1 commercial, 2 industrial, 12 food-retail,
 * 13 hospitality) apply an explicit ×0.3 manual→field damping — ITE measures
 * US suburban auto-oriented sites, and VTPI's TDM encyclopedia documents 2–4×
 * overestimation outside that context; restating rates with the factor
 * visible exposed that the commercial arm sat ~10× BELOW its own cited ITE
 * 820 rate (4 vs 12 trips/100 m² damped). The RESTATED arms (3 school,
 * 4 hospital, 6 hotel, 8 farm, 9 civic + the fixed fixtures) keep the legacy
 * calibration numerically, whose divisors folded their own damping choices in
 * (documented per row). Intentional value changes are covered by golden-delta
 * tests, unchanged arms by parity tests (trip-rates.test.ts).
 *
 * Values are floats end to end — rounding happens ONCE when the final four
 * AADT columns are written (per-generator rounding inflated sums: a hundred
 * 1-dwelling garages rounded up per building overstates the street by ~10 %,
 * /gg Codex). building_type ids mirror engine/noise-compute/src/emission/
 * settlement.rs; keep the two files' class lists in sync.
 */

/** At most one of `dwellings`/`trips` is non-zero. */
export interface BuildingLoad {
  dwellings: number
  trips: number
}

type TripRateArm =
  /** Residential: GFA m² per dwelling; caller applies country trips/dwelling. */
  | { kind: 'dwellings'; gfaPerDwelling: number; capDwellings: number }
  /** Activity: trips per 100 m² of basis, clamped to [minTrips, capTrips]. */
  | { kind: 'trips'; basis: 'gfa' | 'footprint'; per100m2: number; minTrips: number; capTrips: number }
  /** Size-independent small fixture. */
  | { kind: 'fixedTrips'; trips: number }
  /** Structurally uninhabited — generates nothing. */
  | { kind: 'none' }

/**
 * basis 'gfa' = footprint × floors (trip demand scales with usable area);
 * basis 'footprint' = ground floor only — for classes where the POI join types
 * the WHOLE building from a ground-floor venue (a café in a 20-storey tower
 * must not generate 20 floors of restaurant traffic; same reasoning as the
 * engine's `is_shed_type` footprint scaling).
 *
 * Provenance per arm; "restated" = numerically equal to the pre-2026-07
 * divisor arm (old divisor × 3.68 trips/dwelling), pinned by parity tests.
 */
export const BUILDING_TRIP_RATES: Readonly<Record<number, TripRateArm>> = {
  // 0 unknown/apartments — ITE 221; 1 dwelling / 80 m² GFA (docs/about GFA
  // definition), floor+clamp semantics preserved exactly from estimateDwellings.
  0: { kind: 'dwellings', gfaPerDwelling: 80, capDwellings: 200 },
  // 1 commercial/office — ITE 820 shopping 37 trips/1000 ft²/day ≈ 40/100 m²
  // blended with ITE 710 general office ≈ 10/100 m², ×0.3 field damping → 12.
  // GOLDEN DELTA: was 4.0/100 m² (divisor 92 m²/dw), ~10× under its own citation.
  1: { kind: 'trips', basis: 'gfa', per100m2: 12, minTrips: 5, capTrips: 3000 },
  // 2 industrial building — ITE 110 light industrial 4.87/1000 ft² ≈ 5.2/100 m²,
  // ×0.3 → 1.6. GOLDEN DELTA: was 0.54/100 m² (divisor 686).
  2: { kind: 'trips', basis: 'gfa', per100m2: 1.6, minTrips: 4, capTrips: 1500 },
  // 3 school — staff-only rationale (pupils walk/bus), restated (divisor 800).
  3: { kind: 'trips', basis: 'gfa', per100m2: 0.46, minTrips: 4, capTrips: 368 },
  // 4 hospital — ITE 610, 10 trips/bed at ~30 m²/bed, restated (divisor 11).
  4: { kind: 'trips', basis: 'gfa', per100m2: 33.5, minTrips: 20, capTrips: 1104 },
  // 5 church — ITE 560 peak-only, daily minimal; restated (2 dw × 3.68).
  5: { kind: 'fixedTrips', trips: 7.36 },
  // 6 hotel — ITE 310 8.17 trips/room × 0.5 occupancy × 0.6 car mode,
  // room ≈ 25 m²; restated (divisor 38).
  6: { kind: 'trips', basis: 'gfa', per100m2: 9.7, minTrips: 8, capTrips: 1472 },
  // 7 garage — one household's car storage; restated (1 dw × 3.68). Fixed, so
  // amenity=parking areas that classify here stay near-silent by design.
  7: { kind: 'fixedTrips', trips: 3.68 },
  // 8 farm — rural low mobility, restated (divisor 200, cap 50 dw).
  8: { kind: 'trips', basis: 'gfa', per100m2: 1.84, minTrips: 2, capTrips: 184 },
  // 9 civic/public — office-grade day traffic, restated (divisor 300).
  9: { kind: 'trips', basis: 'gfa', per100m2: 1.23, minTrips: 2, capTrips: 368 },
  // 10 SILENT — sheds/roofs/huts/greenhouses. New:
  // previously fell into the residential default arm — ~18 M uninhabited
  // footprints worldwide each generating ~3.7 phantom trips/day.
  10: { kind: 'none' },
  // 11 HOUSE — ITE 210 single-family; larger GFA per dwelling than apartments
  // (garage/attic/setback), capped at 4 units (semidetached/terrace rows).
  // NEW: previously fell into the 0-arm (1 dw / 80 m², cap 200).
  11: { kind: 'dwellings', gfaPerDwelling: 120, capDwellings: 4 },
  // 12 FOOD_RETAIL — supermarket/mall/department_store (incl. area_source
  // functional zones, floors=0 → GFA=footprint). ITE 820 shopping 40/100 m² +
  // ITE 850 supermarket 114/100 m², blend ×0.3 → 30, footprint basis. NEW.
  12: { kind: 'trips', basis: 'footprint', per100m2: 30, minTrips: 20, capTrips: 20000 },
  // 13 HOSPITALITY — restaurant/café/pub/bar/fast_food; ITE 932 high-turnover
  // sit-down 107/1000 ft² ≈ 115/100 m², ×0.3 → 30 (upper-end damping kept:
  // worldwide hospitality is less car-oriented than ITE's US sites). NEW —
  // the Thailand Krabi owner report: guesthouse+restaurant roads carried
  // 3 moto/day because these classes were billed as ordinary houses.
  13: { kind: 'trips', basis: 'footprint', per100m2: 30, minTrips: 15, capTrips: 2000 },
}

/** Footprint fallback when area_m2 is null — one modest small building
 *  (preserved from estimateDwellings). */
const DEFAULT_FOOTPRINT_M2 = 100

/**
 * Building geometry → {dwellings, trips} (at most one non-zero, floats).
 * `floors` 0 means unknown → treated as 1 (single-storey), matching the old
 * `effectiveFloors` rule; unknown building_type falls to the residential arm
 * exactly as before (arm 0 IS the `default` of the old switch).
 */
export function estimateBuildingLoad(
  buildingType: number,
  floors: number,
  areaM2: number | null,
): BuildingLoad {
  const footprint = areaM2 ?? DEFAULT_FOOTPRINT_M2
  const effectiveFloors = floors > 0 ? floors : 1
  const gfa = footprint * effectiveFloors
  const arm = BUILDING_TRIP_RATES[buildingType] ?? BUILDING_TRIP_RATES[0]
  switch (arm.kind) {
    case 'dwellings': {
      // floor()+min 1+cap — bit-parity with the historical integer dwellings.
      const dw = Math.min(Math.max(1, Math.floor(gfa / arm.gfaPerDwelling)), arm.capDwellings)
      return { dwellings: dw, trips: 0 }
    }
    case 'trips': {
      const size = arm.basis === 'gfa' ? gfa : footprint
      const raw = (size / 100) * arm.per100m2
      return { dwellings: 0, trips: Math.min(Math.max(raw, arm.minTrips), arm.capTrips) }
    }
    case 'fixedTrips':
      return { dwellings: 0, trips: arm.trips }
    case 'none':
      return { dwellings: 0, trips: 0 }
  }
}
