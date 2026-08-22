---
title: Brazil
intro: Noise mapping data sources for Brazil.
map: { center: [-53, -14], zoom: 4 }
---

## Road traffic

### DNIT federal highways (ArcGIS Online mirror)

Brazil's federal highway geometry comes from **DNIT** (Departamento Nacional de Infraestrutura de Transportes) via a public ArcGIS Online mirror:

- **Source**: `services3.arcgis.com/KYEMegXJrTiWSYWk/arcgis/rest/services/RODOVIAS_FEDERAIS_DNIT_2017/FeatureServer/0`
- **Records**: 7,607 polyline segments of Brazilian federal highways (BR-series)
- **Vintage**: 2017 (registered as "DNIT Rodovias Federais", proxy measurement — not per-segment counts)
- **Fields**: `Codigo_BR` (BR number, e.g. 101, 116, 040), `Unidade_Fe` (state), `Superficie` (PAV = paved, IMP = unpaved, PLA = planned, OBR = under construction), `Administra` (Federal / Concessionada)

**No per-segment AADT is available for Brazil**: DNIT runs a national traffic count program (PNCT, `servicos.dnit.gov.br`) but it is geo-blocked from non-Brazilian IPs. Instead, OSM motorway/trunk/primary segments within 400 m of a DNIT federal highway receive a class-based estimate by surface + administration type, scaled by metro tier:

| DNIT combination | AADT (rural) |
|---|---:|
| Paved (PAV) + Concessionada (toll concession) | 35,000 |
| Paved (PAV) + Federal | 25,000 |
| Unpaved / planned / under construction | 3,000 |

Roads with no DNIT match are **not stamped** — they fall through to the engine's country-tier default cascade (below).

### Metro tiers

**Tier-1 metros** (×2.0): **São Paulo** (Grande SP), **Rio de Janeiro**, **Brasília**, **Belo Horizonte**.

**Tier-2 cities** (×1.4, 31 cities): Salvador, Fortaleza, Curitiba, Manaus, Recife, Porto Alegre, Goiânia, Belém, Guarulhos, Campinas, São Luís, Maceió, Natal, Campo Grande, Teresina, João Pessoa, Nova Iguaçu, São Bernardo, Santo André, Osasco, Ribeirão Preto, Uberlândia, Sorocaba, São José dos Campos, Niterói, Contagem, Joinville, Aracaju, Cuiabá, Florianópolis, Vitória.

### Class defaults (engine cascade)

Everything not matched to a DNIT federal highway resolves through the engine's traffic-default cascade (city → country → continent → world). Brazil carries explicit cascade arms derived from DNIT 2023 federal AADT estimates + IBGE metro tiers (DNIT traffic portal: `servicos.dnit.gov.br/vmt/`):

| OSM class | BR rural (country default) | São Paulo / Rio (metro default) |
|---|---:|---:|
| 0 motorway (rodovia dual-carriageway) | 50,000 | 100,000 |
| 1 trunk (BR-series) | 25,000 | 50,000 |
| 2 primary | 12,000 | 24,000 |
| 3 secondary | 5,000 | 10,000 |
| 4 tertiary | 2,000 | 4,000 |
| 5 residential | 1,000 | 2,000 |
| 6 living_street | 400 | — |

Only São Paulo and Rio de Janeiro carry engine-level metro defaults. Hexes in Brasília, Belo Horizonte and the tier-2 cities fall back to the BR rural column unless the segment was DNIT-matched (where the ×2.0 / ×1.4 tier multipliers do apply). Residential and service roads additionally get the global dwelling-based service-tree heuristic.

### Brazilian vehicle split

Brazil has a low motorcycle share (~5%, lower than Southeast Asia) but a much higher heavy-vehicle share on rural corridors — agricultural freight, iron ore, soy, mining:

| Tier | Light | Medium | Heavy | Motorcycle |
|---|---:|---:|---:|---:|
| Tier-1 / Tier-2 | 70% | 10% | 15% | 5% |
| Rural | 60% | 10% | **25%** | 5% |

## Railway

### No Brazil-specific rail enricher — engine class defaults

There is **no Brazilian rail enricher**. No open per-line train-frequency dataset is ingested — the major metro and suburban operators (Metrô de São Paulo, CPTM) do not consistently publish GTFS, a gap shared across South America. An earlier class-default rail pass over Brazilian rail geometry was removed as pseudo-enrichment (it added no information over the engine defaults). Brazilian rail therefore uses the engine's generic CNOSSOS class defaults, keyed only by OSM rail type + usage:

| rail_type | usage | pax/day | frt/day |
|---|---|---:|---:|
| rail | main | 80 | 20 |
| rail | branch | 30 | 5 |
| rail | industrial siding | 0 | 15 |
| rail | unknown | 40 | 10 |
| tram | — | 120 | 0 |
| light_rail | — | 80 | 0 |
| narrow gauge | — | 10 | 0 |

These defaults are not tuned to Brazilian operations — a heavy iron-ore freight corridor runs far more than 20 freight trains/day, and a São Paulo metro line far more than 80 passenger services. OSM `railway=subway` lines are additionally not extracted (a global extraction limitation, shared with e.g. Tokyo and Seoul).

## Buildings

GHSL Built-H R2023A 100 m heights + Overture Maps Foundation global footprints. Building heights across South American cities come from the GHSL raster (Overture has <1% height coverage for SA cities). No Brazilian cadastre or height dataset is ingested.

## Industrial

### SIGACONTROL — ANEEL energy fleet via ArcGIS Online

Brazil has the richest power-sector geometry in this pipeline. ANEEL (the national electricity regulator) publishes its SIGA registry via ArcGIS Online (`services5.arcgis.com/qaWxR4XTuVOZEXZ9`):

| Layer | Records | Notes |
|---|---:|---|
| Aerogeradores_Brasil | **11,182 individual wind turbines** | with rated MW, total height, hub height and rotor diameter — the richest wind turbine dataset in the entire pipeline |
| UsinaTermoeletrica | 3,226 thermal plants | coal / gas / oil / biomass |
| Aproveitamento_Hidroletrico | 1,138 hydro plants | including Itaipu, Belo Monte, Tucuruí, Furnas, Itumbiara |
| UsinaFotovoltaica | 322 solar PV plants | |
| UsinaTermonuclear | 3 nuclear plants | Angra I, II, III |

Only operational plants are stamped (`OPERACAO = SIM` for turbines, `ESTAGIO` = operação for other plants). All map to NACE 35, resolving the corresponding industrial base profiles before shared spectrum and area terms. Each OSM industrial site gets its nearest plant within 2 km using fixed input precedence (thermal → hydro → nuclear → solar; wind excluded). This national registry overrides the global GPPD power-plant baseline (255 BR plants).

### Wind turbines

Wind turbines are modelled as a separate source type from OSM turbine points — they are deliberately never stamped as industrial plants (a turbine inheriting a power-plant NACE code would be wildly over-loud). The SIGA per-turbine specs (hub height, rotor diameter, rated MW) are downloaded but **not yet merged onto OSM turbines** — the per-turbine spec matcher currently exists only for the US (USWTDB). Brazilian turbines therefore rely on OSM tags for their emission power class.

### Brazil does NOT have

- **No per-segment AADT** — DNIT PNCT traffic counts are geo-blocked from non-BR IPs; road volumes are class-based estimates, not measurements
- **No rail traffic data** — no GTFS or timetable ingest; rail runs on engine class defaults
- **No mining or oil & gas registry** — unlike Colombia (ANM/ANH), Chile (SERNAGEOMIN) and Peru (INGEMMET); Brazilian mining noise relies on OSM industrial land use only
- **No building heights beyond GHSL 100 m** — no cadastre ingest
- **No per-turbine spec merge** — SIGA turbine metadata (hub height, rated MW) is not yet applied to OSM turbines

## Validation

Brazil does not implement END (the EU Environmental Noise Directive) — South America has no equivalent continental mandate for open noise maps. The national framework is CONAMA Resolução 001/1990 with the ABNT NBR 10151 measurement standard, enforced at municipal level. No official Brazilian strategic noise map is wired into the pipeline as a validation reference.

Notable noise zones:

- **BR-101, BR-116, BR-040** — flagship federal corridors present in the DNIT dataset (`Codigo_BR` field)
- **Grande São Paulo, Rio de Janeiro, Brasília, Belo Horizonte** — tier-1 metro road networks (×2.0 traffic multiplier)
- **Itaipu** (14 GW, bi-national with Paraguay) — flagged as Brazil in the plant data
- **Belo Monte, Tucuruí, Furnas, Itumbiara** — major hydro plants (90 dB Lw class, 24/7 baseload)
- **Angra I/II/III nuclear complex** (97 dB Lw class, near-24/7)
- **11,182-turbine wind fleet** (SIGA) — modelled as individual turbine sources
- **Guarulhos, Congonhas (São Paulo), Galeão, Santos Dumont (Rio de Janeiro), Brasília** — airports covered by the global aircraft layer
