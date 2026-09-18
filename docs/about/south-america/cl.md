---
title: Chile
intro: MOP traffic census stations 2024 and 2025 on the state road network, estimates from Red Vial classification elsewhere. Power plants, tailings dams and substations from Chilean registers. No train timetable is loaded.
map: { center: [-71, -35], zoom: 4 }
---

## Roads

Traffic volumes: station counts (TMDA) of the Plan Nacional de Censos, published by the Ministerio de Obras Públicas on its [map server](https://rest-sit.mop.gob.cl/); 2024 and 2025 stations. A station counts each branch of its junction, and the busiest branch is used. An OSM motorway, trunk or primary road takes the nearest station within 600 m. Stations are points, so roads far from any station receive no census value.

Those roads are matched within 400 m to the MOP Red Vial network and estimated from its classification:

| Road in the Red Vial | Vehicles per day |
|---|---:|
| Paved and under concession (toll) | 35,000 |
| Paved national longitudinal (Ruta 5) | 22,000 |
| Paved national | 14,000 |
| Paved regional principal | 7,000 |
| Paved provincial | 3,500 |
| Other paved | 2,000 |
| Gravel or dirt | 1,500 |

Both counts and estimates are doubled inside Greater Santiago and multiplied by 1.4 in 24 other cities: Valparaíso, Viña del Mar, Concepción, Talcahuano, La Serena, Coquimbo, Antofagasta, Iquique, Arica, Temuco, Rancagua, Talca, Chillán, Puerto Montt, Osorno, Valdivia, Calama, Copiapó, Punta Arenas, Curicó, Los Ángeles, San Antonio, Quillota and Tomé. The city boxes are drawn manually.

The vehicle mix is an estimate:

| Where | Light | Medium | Heavy | Motorcycle |
|---|---:|---:|---:|---:|
| Santiago | 75% | 10% | 10% | 5% |
| The 24 cities | 73% | 10% | 12% | 5% |
| The north, outside cities | 50% | 10% | 38% | 2% |
| Everywhere else | 60% | 10% | 27% | 3% |

"The north" is a single box from Arica to La Serena; every road in it outside the cities takes the 38% heavy share.

Secondary and smaller roads use world defaults.

## Railways

No timetable is loaded, neither for the EFE commuter trains around Santiago, Valparaíso and Concepción nor for the freight railways of the north. All lines use class defaults: 80 passenger and 20 freight trains per day on a main line, 30 and 5 on a branch, 15 freight trains on a line tagged industrial, 10 trains on narrow gauge. The Santiago Metro is tagged railway=subway and is not included.

## Industry

Sources:

- thermal power plants from the Comisión Nacional de Energía, operating plants only
- other power plants from the Global Energy Monitor power tracker
- tailings dams from the SERNAGEOMIN register, active or under construction, classed as metal ore mining
- substations of 110 kV and above from the CNE transmission data

Each record is attached to an OSM industrial area within 2 km; a record with no such area nearby is not a noise source. Mines are in none of these registers: an open pit is an OSM area with a type inferred from name and tags.

Wind turbines are OSM points; a turbine without tagged specs is treated as a 2 MW machine.

## Ships

Global Fishing Watch AIS vessel density.
