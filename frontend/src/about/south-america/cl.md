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

Counts are used as published. Estimates are doubled inside Greater Santiago and multiplied by 1.4 in 24 other cities: Valparaíso, Viña del Mar, Concepción, Talcahuano, La Serena, Coquimbo, Antofagasta, Iquique, Arica, Temuco, Rancagua, Talca, Chillán, Puerto Montt, Osorno, Valdivia, Calama, Copiapó, Punta Arenas, Curicó, Los Ángeles, San Antonio, Quillota and Tomé. The city boxes are drawn manually.

The vehicle mix is an estimate:

Vehicle mix: where not counted, the medium and heavy vehicles that counted roads of the same class carry; motorcycles 0.7 % of a town street's traffic (WHO 2023); electric cars 0.2 % of cars, rolling noise only (IEA 2024).

Secondary and smaller roads carry the trips their buildings make, else world defaults.

Unsigned roads: a primary road 50 km/h in towns, 70 outside.

## Railways

No timetable is loaded, neither for the EFE commuter trains around Santiago, Valparaíso and Concepción nor for the freight railways of the north. All lines use the [world railway defaults](/about/methodology). Surface metro sections are included; see the [railway method](/about/methodology).

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

## Homes and places

Homes mapped in OpenStreetMap: 16 air conditioners per 100 homes (the region's median). Mosques: only those with a mapped minaret call, once on Fridays.
