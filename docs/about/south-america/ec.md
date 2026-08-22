---
title: Ecuador
intro: Noise mapping data sources for Ecuador.
map: { center: [-78, -1.5], zoom: 6 }
---

## Road traffic

### CONGOPE Red Vial Ecuador

Ecuador's MTOP (Ministerio de Transporte y Obras Públicas) and ANT (Agencia Nacional de Tránsito) gov portals are TCP-blocked from non-EC IPs. The only working data path is via **CONGOPE** (Consorcio de Gobiernos Provinciales del Ecuador) mirrors on ArcGIS Online, which host both the national Red Vial Estatal and a comprehensive 28,328-segment network combining 24 GAD provincial networks.

- **Source 1 — Red Vial Estatal**: `services6.arcgis.com/pYn2F4v1aESZqj1u/arcgis/rest/services/Red_Vial_Estatal/FeatureServer/0`
  - **711 polylines** — the national state road network (RVE)
  - Fields: `CLASIFICAC` (ARTERIAL 457 / COLECTORA 254), `ESTADO` (BUENO/REGULAR/CIRCULE CON PRECAUCIÓN/MUY BUENO), `NOMBRE_TRA`, `PROVINCIAS`

- **Source 2 — Red Vial Ecuador (comprehensive)**: `services6.arcgis.com/pYn2F4v1aESZqj1u/arcgis/rest/services/Red_Vial_Ecuador/FeatureServer/0`
  - **28,328 polylines** — national + 24 GAD provincial networks combined
  - Fields: `ADMINISTRA` (directa MTOP vs GAD provincial), `TIPO_VIA`, `TIPO_CALZA`, `ANCHO_CAL`, `ESTADO`, `PROVINCIA`

**Ecuador publishes NO per-segment IMD/AADT in machine-readable form.** Fall back to classification-based defaults — as in Bolivia, Paraguay and Venezuela (only AR/CL/CO/PE carry surveyed per-segment counts).

### Ecuadorian AADT defaults (Costa/Sierra/Oriente regional split)

| OSM class | Costa | Sierra | Oriente | Tier-1 (×2.0) | Tier-2 (×1.4) |
|---|---:|---:|---:|---:|---:|
| 0 motorway | 30,000 | 20,000 | 6,000 | 60,000 | 42,000 |
| 1 trunk | 12,000 | 8,000 | 3,000 | 24,000 | 16,800 |
| 2 primary | 6,000 | 4,000 | 1,500 | 12,000 | 8,400 |
| 3 secondary | 3,000 | 2,000 | 800 | 6,000 | 4,200 |
| 4 tertiary | 1,500 | 1,000 | 400 | 3,000 | 2,100 |
| 5 residential | 700 | 500 | 200 | 1,400 | 980 |

### CONGOPE Red Vial spatial-match AADT

| CONGOPE combination | AADT (rural) |
|---|---:|
| Red Vial Estatal ARTERIAL paved (E15/E25/E35) | 15,000 |
| Red Vial Estatal COLECTORA paved | 6,000 |
| Red Vial Estatal unpaved | 2,000 |
| Red Vial Ecuador GAD provincial paved | 4,000 |
| Red Vial Ecuador GAD unpaved | 1,200 |

**Tier-1 metros** (×2.0): **Quito D.M.** (~1.8M, 2,850m altitude, capital) and **Guayaquil** (~2.6M, coastal, Ecuador's largest city).

**Tier-2 cities** (×1.4, 17 cities): Cuenca, Santo Domingo, Machala, Durán, Manta, Portoviejo, Ambato, Loja, Riobamba, Esmeraldas, Ibarra, Latacunga, Milagro, Babahoyo, Quevedo, Lago Agrio, Tulcán.

### Ecuadorian vehicle split

Ecuador has moderate motorcycle share (~15% urban). Heavy share elevated on oil freight routes (Amazon → SOTE pipeline → Esmeraldas/Balao terminals) and banana/flower export routes to Guayaquil port.

| Tier | Light | Medium | Heavy | Motorcycle |
|---|---:|---:|---:|---:|
| Tier-1 (Quito/Guayaquil) | 67% | 6% | 12% | 15% |
| Tier-2 | 68% | 6% | 12% | 14% |
| Costa rural | 60% | 8% | 22% | 10% |
| Sierra | 55% | 8% | 27% | 10% |
| **Oriente (Amazon)** | 50% | 8% | **32%** | 10% |
| **Oil freight corridor (Sucumbíos/SOTE)** | 45% | 8% | **37%** | 10% |

### National route network

- **E25 Panamericana** — main Andean spine: Tulcán (Colombia border) ↔ Ibarra ↔ Quito ↔ Latacunga ↔ Ambato ↔ Riobamba ↔ Cuenca ↔ Loja ↔ Peru border
- **E15 Vía del Pacífico** — coastal highway: Mataje (Colombia border) ↔ Esmeraldas ↔ Manta ↔ Guayaquil ↔ Salinas ↔ Peru border
- **E35** — alternate Andean spine
- **E20** — Quito ↔ Esmeraldas (Calacalí-La Independencia)
- **E10 / E45** — Lago Agrio ↔ Francisco de Orellana (Coca) — Amazon oil corridor
- **E40** — Quito ↔ Tena ↔ Puyo (Amazon access)
- **Quito ↔ Guayaquil** — the two largest metros, freight corridor via Santo Domingo

## Railway

Ecuador has **no bespoke rail enricher** — only Argentina does in South America. Ecuadorian rail noise is computed from **OSM rail geometry with class-default passenger/freight frequencies** (the table below), not from an ingested rail or transit feed.

### Metro de Quito Line 1 (opened December 2023 — Ecuador's first metro)

- **Metro de Quito Line 1**: 22 km underground, Quitumbe ↔ El Labrador, opened December 2023. Designed for ~380,000 daily passengers. Operator: Empresa Metro de Quito. A `Metro_de_Quito` ArcGIS layer (station points + Line 1 alignment) exists but is **not ingested**. OSM `railway=subway` is excluded from the surface-rail extract, so the underground metro does not contribute to this layer.

### Ecuadorian rail context

Ecuador has extremely limited operational rail:

- **Metro de Quito Line 1** — **OPENED DECEMBER 2023, Ecuador's first and only operating metro**
- **Ferrocarriles del Ecuador (EFE)** — mostly tourist since 2020. Pre-2020 operations: **Tren Crucero** (Quito ↔ Guayaquil 4-day Andes tour), **Devil's Nose switchbacks** at Alausí (famous zig-zag descent), various short tourist excursions. Currently very limited operations.
- **No freight rail** — Ecuador has no operational freight rail corridors.
- **Aerovía Guayaquil/Durán** — cable car across Guayas river (opened 2020), `aerialway` not rail.
- **Metrovía Guayaquil + Trolebus/Ecovía Quito** — BRT bus, NOT rail.

### Rail defaults

No measured/GTFS frequencies, so rail uses the engine's per-type class defaults
(identical worldwide): main line 80 pax + 20 freight/day, branch 30/5, industrial
siding 0/15, unknown 40/10, tram 120/0, light rail 80/0, narrow gauge 10/0,
funicular 40/0. Country-specific counts need GTFS or measured data.

## Buildings

GHSL Built-H R2023A 100m + Overture Maps Foundation global footprints. Microsoft contributed Ecuadorian building footprints in their 2023-2024 release. No EC-specific cadastre — IGM Ecuador publishes maps via Wordpress only (`geoportaligm.gob.ec`), no Portal/REST endpoint.

## Industrial

### GEM Global Integrated Power — 165 plants, 70 operating

Ecuador's ARCOM (mining) and ARCONEL (energy) regulators don't publish machine-readable facility registries. All previously-indexed ARCOM catastro mirrors on ArcGIS Online have been deleted. **GEM is the only usable source.**

- **Source**: `services.arcgis.com/P3ePLMYs2RVChkJx/arcgis/rest/services/Global_Integrated_Power_v1/FeatureServer/0?where=Country_area='Ecuador'`

Ecuador gets ~70% of electricity from hydropower, with the remaining from gas CCGT (Guayaquil area), small diesel, solar, wind.

### Top operating plants

| Plant | MW | Type | Location |
|---|---:|---|---|
| **Coca Codo Sinclair** | 1,500 | hydropower | Napo (Amazon, CELEC EP, China-built) |
| **Paute Molino** | 1,075 | hydropower | Azuay (Paute cascade, CELEC EP) |
| **Sopladora** | 487 | hydropower | Morona Santiago (Paute cascade, CELEC EP) |
| Minas San Francisco | 276 | hydropower | Azuay (CELEC EP) |
| San Francisco (Ecuador) | 230 | hydropower | Tungurahua (CELEC EP) |
| Marcel Laniado | 213 | hydropower | Guayas (CELEC EP) |
| Toachi-Alluriquin | 204 | hydropower | Pichincha/Santo Domingo |
| Delsitanisagua | 180 | hydropower | Zamora-Chinchipe |
| **Paute Mazar** | 170 | hydropower | Azuay (Paute cascade, CELEC EP) |
| **Paute Agoyán** | 160 | hydropower | Tungurahua |

**Operating fuel breakdown**: hydropower 34, solar 18, oil/gas 14, bioenergy 2, wind 2.

All mapped to **NACE 35** (Electricity generation).

### Ecuador does NOT have

- **No open mining concession cadastre** — ARCOM publishes only via the (blocked) gov portal. All community mirrors have been deleted. Major mines (**Mirador** copper, **Fruta del Norte** gold, **Cascabel** exploration) rely on OSM coordinates only.
- **No Petroecuador refinery database** — Esmeraldas (~110k bpd), La Libertad (~45k bpd, Santa Elena), and Shushufindi refineries are tagged via OSM `landuse=industrial` with the generic OSM industrial base profile and no NACE override.
- **No per-segment IMD/AADT** — the only SA country in pipeline without ANY real traffic measurements.

## Validation

Ecuador implements noise regulation via:

- **MAAE** (Ministerio del Ambiente, Agua y Transición Ecológica) at [ambiente.gob.ec](https://www.ambiente.gob.ec/)
- **Acuerdo Ministerial 097-A** (2015) — Norma Técnica para emisión de ruido
- **Texto Unificado de Legislación Secundaria de Medio Ambiente (TULSMA)** — noise standards under Libro VI, Anexo 5:
  - Residential day/night: 55/45 dBA
  - Mixed: 60/50 dBA
  - Commercial: 65/55 dBA
  - Industrial: 70/65 dBA
- **MDMQ** (Municipio del Distrito Metropolitano de Quito) — Quito-specific enforcement with stricter day/night 60/50 dBA
- **Municipalidad de Guayaquil** — equivalent local enforcement

Notable noise zones:

- **E25 Panamericana** — main Andean spine through Tulcán/Ibarra/Quito/Latacunga/Ambato/Riobamba/Cuenca/Loja
- **E15 Vía del Pacífico** — coastal highway
- **Quito ↔ Santo Domingo ↔ Guayaquil corridor** — major freight artery
- **Av. General Rumiñahui / Av. Eloy Alfaro / Av. Amazonas / Av. NN.UU.** Quito — major arterials
- **Vía a la Costa / Av. Benjamín Carrión / Perimetral** Guayaquil — urban expressways
- **Quito Metro Line 1** — underground, Quitumbe ↔ El Labrador (opened December 2023)
- **SOTE + OCP oil pipelines** — Amazon (Sucumbíos) ↔ Esmeraldas/Balao terminals
- **Mariscal Sucre (UIO/SEQM Quito)**, **José Joaquín de Olmedo (GYE/SEGU Guayaquil)**, **Mariscal Lamar (CUE/SECU Cuenca)**, **Camilo Ponce Enríquez (LOH/SETM Latacunga-actually Cotopaxi)**, **Cotopaxi Latacunga (LTX/SELT)**, **Eloy Alfaro (MEC/SEMT Manta)**, **Seymour (GPS/SEGS Galápagos Baltra)**, **San Cristóbal (SCY/SEST Galápagos)** — covered by global aircraft layer
- **Coca Codo Sinclair Hydroelectric Dam** (Napo, 1,500 MW) — Ecuador's largest power plant
- **Paute cascade** (Azuay/Morona Santiago: Paute Molino 1,075 + Sopladora 487 + Mazar 170 + Mazar-Dudas + Cardenillo) — total ~1.9 GW
- **Refinería Esmeraldas** (Petroecuador, ~110k bpd) — Ecuador's largest refinery
- **Refinería La Libertad** (Petroecuador, Santa Elena, ~45k bpd)
- **Shushufindi refinery** (Sucumbíos, near Amazon oil field)
- **Mirador copper mine** (Zamora-Chinchipe, Ecuacorriente/CRCC) — Ecuador's first large-scale modern copper mine
- **Fruta del Norte gold mine** (Zamora-Chinchipe, Lundin Gold) — major gold producer
- **Quito-Guayaquil freight route** — banana/flower export corridor
