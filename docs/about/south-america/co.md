---
title: Colombia
intro: INVIAS 2024 traffic census with a measured bus and truck share on national roads, estimates from road administration elsewhere. No train timetable is loaded.
map: { center: [-74, 4], zoom: 5 }
---

## Roads

Traffic volumes: the INVIAS 2024 traffic census, published as an [open map service](https://services6.arcgis.com/kyerLIHvrND0OSya/). Each counted section has TPDA (average daily traffic) and the percentage of cars, buses and trucks. Motorcycles are not counted; a flat 5% is added and the measured shares are scaled to 95%.

An OSM motorway, trunk or primary road takes the nearest counted section within 500 m. Sections under 50 vehicles per day are ignored.

Roads without a nearby count are matched within 400 m to the INVIAS national road network and estimated from the administering body:

| Road in the national network | Vehicles per day |
|---|---:|
| Paved ANI concession, dual carriageway | 25,000 |
| Paved ANI concession, single carriageway | 18,000 |
| Paved INVIAS road | 12,000 |
| Other paved road | 6,000 |
| Unpaved | 1,500 |

Both counts and estimates are doubled inside Bogotá and Medellín and multiplied by 1.4 in 24 other cities: Cali, Barranquilla, Cartagena, Cúcuta, Bucaramanga, Pereira, Santa Marta, Ibagué, Manizales, Pasto, Villavicencio, Neiva, Armenia, Soledad, Soacha, Valledupar, Montería, Sincelejo, Buenaventura, Tunja, Riohacha, Quibdó, Florencia and Popayán. The city boxes are drawn manually.

Roads with a network estimate also take an estimated vehicle mix:

| Where | Light | Medium | Heavy | Motorcycle |
|---|---:|---:|---:|---:|
| Bogotá, Medellín | 55% | 5% | 10% | 30% |
| The 24 cities | 60% | 5% | 10% | 25% |
| Coal regions of La Guajira and Cesar | 40% | 5% | 45% | 10% |
| Everywhere else | 55% | 8% | 22% | 15% |

Secondary and smaller roads use world defaults.

## Railways

No timetable is loaded; the Cerrejón and FENOCO coal railways publish no schedule. All lines use the [world railway defaults](/about/methodology).

Metro de Medellín takes the light rail default of 80 trains per day where OSM tags it as light rail. Surface metro sections are included; see the [railway method](/about/methodology).

## Industry

Power plants: Global Energy Monitor power tracker, operating plants only. Each is attached to an OSM industrial area within 2 km; a plant with no such area nearby is not a noise source.

Mines, oil fields and refineries are OSM industrial areas with a type inferred from name and tags. The open mining titles of the Agencia Nacional de Minería and the production blocks of the Agencia Nacional de Hidrocarburos are not loaded.

## Ships

Both coasts: Global Fishing Watch AIS vessel density.
