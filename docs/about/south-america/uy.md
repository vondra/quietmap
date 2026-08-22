---
title: Uruguay
intro: Noise mapping data sources for Uruguay.
map: { center: [-56, -33], zoom: 6 }
---

## Road traffic

### Road defaults

Uruguay publishes no open per-segment AADT, so roads fall back to the global class defaults scaled by Uruguay's traffic factor **≈ 0.722** (population density). Only the major classes (motorway, trunk, primary, and their on/off-ramps) are scaled; local roads and the vehicle mix use the global default — the engine applies no per-city tiers or country-specific splits.

| OSM class | Default AADT |
|---|---:|
| Motorway | 30,000 × 0.722 ≈ 21,660 |
| Trunk | 15,000 × 0.722 ≈ 10,830 |
| Primary | 9,000 × 0.722 ≈ 6,498 |
| Secondary / tertiary / residential | 3,000 / 800 / 500 (world default) |
| Service / track / unclassified | 250 / 5 / 1,340 (world default) |

### National route network

- **Ruta 1** — Montevideo ↔ Colonia del Sacramento ↔ Argentina (220 km, ferry to Buenos Aires)
- **Ruta 2** — Montevideo ↔ Fray Bentos via Mercedes (Argentina border)
- **Ruta 3** — Montevideo ↔ Artigas (Brazil border, northwest, ~800 km central spine)
- **Ruta 5** — Montevideo ↔ Rivera (Brazil border, north, via Durazno/Tacuarembó)
- **Ruta 8** — Montevideo ↔ Melo (Brazil border, east)
- **Ruta 9** — Montevideo ↔ Punta del Este ↔ Brazil border (southeast, coastal)
- **Ruta Interbalnearia (IB)** — Montevideo ↔ Punta del Este coastal expressway (Uruguay's only autopista-class road)

## Railway

### No spatial data available — defaults only

MTOP GeoServer rail layers (`via_prin_act`, `afe_velocidad_y_cargas`, `afe_estaciones`) are blocked. Apply geographic defaults via OSM rail geometry + Ferrocarril Central + Montevideo bboxes.

### Uruguayan rail context

- **Ferrocarril Central** — brand-new 273 km railway **opened 2023** for **UPM Paso de los Toros pulp mill (UPM2)**. Connects Paso de los Toros (central interior) to the Montevideo deep water terminal. **Uruguay's largest infrastructure project in decades** (~US$1 billion). Heavy freight: wood chips, pulp bales, chemicals. Standard gauge (1,435 mm) distinct from legacy AFE broad gauge (1,676 mm).
- **AFE (Administración de Ferrocarriles del Estado)** — state railway. Limited freight + small Montevideo suburban passenger service. The long-distance lines (Montevideo ↔ Salto) are mostly inactive since the 1980s reforms.
- **No urban metro/light rail** — Montevideo uses buses only (STM Sistema de Transporte Metropolitano).

### Rail defaults

No measured/GTFS frequencies, so rail uses the engine's per-type class defaults
(identical worldwide): main line 80 pax + 20 freight/day, branch 30/5, industrial
siding 0/15, unknown 40/10, tram 120/0, light rail 80/0, narrow gauge 10/0,
funicular 40/0. Country-specific counts need GTFS or measured data.

## Buildings

GHSL Built-H R2023A 100m + Overture Maps Foundation global footprints. Microsoft contributed Uruguayan building footprints in their 2023-2024 release.

## Industrial

### GEM Global Integrated Power — 78 plants, 73 operating

- **Source**: `services.arcgis.com/P3ePLMYs2RVChkJx/arcgis/rest/services/Global_Integrated_Power_v1/FeatureServer/0?where=Country_area='Uruguay'`

Uruguay generates **~95% of electricity from renewables** (wind + hydro + solar + biomass) — the highest renewable share in the world for a country of Uruguay's size. Uruguay also has the **world's highest wind penetration per capita**.

**Top operating plants**:

| Plant | MW | Type | Location |
|---|---:|---|---|
| **Punta del Tigre B** | 540 | oil/gas CCGT | San José (2019) — Uruguay's largest thermal |
| **Constitución/Palmar** | 333 | hydropower | Soriano (Río Negro, 1982) |
| **UPM Paso de los Toros** | 2×155 | bioenergy | Tacuarembó (2023) — UPM2 pulp mill |
| **Rincón del Bonete (Gabriel Terra)** | 152 | hydropower | Tacuarembó (Río Negro, 1945) |
| Pampa (Nordex) | 142 | wind | Tacuarembó (2016) |
| **Rincón de Baygorria** | 108 | hydropower | Durazno (Río Negro, 1960) |
| UTE Central Termica La Tablada | 2×106 | oil/gas | Montevideo (1992) |
| **Montes del Plata** | 2×90 | bioenergy | Colonia (2014) — pulp mill co-gen |
| **UPM Fray Bentos** | 80 | bioenergy | Río Negro (2007) — UPM1 pulp mill |

**Operating fuel breakdown** (inside UY bbox): wind 35, solar 14, oil/gas 10, hydropower 3, bioenergy 2.

All mapped to **NACE 35** (Electricity generation).

### Pulp mills — Uruguay's only heavy industry

Uruguay has three mega-mills built in the 21st century:

1. **UPM Fray Bentos** (original UPM1, 2007) — 1.1 Mt pulp/year on Río Uruguay. Sparked the 2006-2010 "Gualeguaychú conflict" with Argentina.
2. **Montes del Plata** (Stora Enso + Arauco JV, 2014) — 1.3 Mt pulp/year at Conchillas on Río de la Plata.
3. **UPM2 Paso de los Toros** (2023) — 2.1 Mt pulp/year at Paso de los Toros. **Uruguay's largest industrial investment ever (~US$3 billion)** + 273 km Ferrocarril Central railway to Montevideo port.

GEM tags the co-gen power generation side of all three as NACE 35. The actual pulp processing (NACE 17 Manufacture of paper) is not separately classified.

### Uruguay does NOT have

- **No open TPDA/AADT data** — MTOP GeoServer is blocked
- **No mining concessions** — modest mining (limestone, granite, dolomite) without open registry
- **No ANCAP refinery NACE classification** — La Teja refinery (Montevideo, ~50k bpd) remains tagged via OSM industrial with the generic OSM industrial base profile and no NACE override
- **No metro/light rail** — Montevideo uses buses only

## Validation

Uruguay implements noise regulation via:

- **Ministerio de Ambiente** at [ambiente.gub.uy](https://www.gub.uy/ministerio-ambiente/)
- **DINAMA** (Dirección Nacional de Medio Ambiente, now part of Ministerio de Ambiente)
- **Ley 17.852 / 2004** — Ley de Prevención y Control de Contaminación Acústica (noise pollution control)
- **Decreto 532/2009** — reglamentary standards:
  - Residential day/night: 55/45 dBA (DINAMA baseline, more stringent in Intendencia de Montevideo bylaws)
  - Mixed: 60/50 dBA
  - Commercial: 65/55 dBA
  - Industrial: 70/65 dBA
- **Intendencia de Montevideo** — city-level enforcement via local decree (stricter nighttime limits)

Notable noise zones:

- **Ruta Interbalnearia (IB)** — Montevideo ↔ Punta del Este (140 km autopista, tourist corridor)
- **Ruta 1** — Montevideo ↔ Colonia (Argentina ferry port)
- **Rambla Costera** Montevideo — 22 km waterfront arterial (one of the longest continuous urban promenades in the world)
- **Av. 18 de Julio, Av. Italia, Av. 8 de Octubre** — Montevideo major arterials
- **Accesos a Montevideo** (Ruta 1, Ruta 8) — major commute routes
- **Ferrocarril Central** — Paso de los Toros ↔ Montevideo (273 km UPM2 pulp corridor, opened 2023)
- **Carrasco (MVD/SUMU Montevideo)**, **Laguna del Sauce (PDP/SULS Punta del Este)**, **Santa Bernardina (DZO/SUDU Durazno)**, **Melo (MLZ/SUMO)**, **Tydings (CYR/SUSO Carmelo)** — covered by global aircraft layer
- **Salto Grande Hydroelectric Dam** (1,890 MW, shared with Argentina on Río Uruguay) — largest power plant in the binational region
- **Constitución/Palmar** (333 MW hydro, Río Negro)
- **UPM Paso de los Toros pulp complex** (Tacuarembó) — Uruguay's largest industrial investment ever
- **Montes del Plata** (Conchillas, Colonia) — Stora Enso + Arauco pulp mill
- **UPM Fray Bentos** (Río Negro) — original UPM pulp mill
- **Punta del Tigre thermal complex** (San José, 540 + 300 MW CCGT) — Uruguay's largest thermal
- **ANCAP La Teja refinery** (Montevideo) — Uruguay's only oil refinery (~50k bpd)
- **Port of Montevideo** — container + cruise terminal, deep water port
