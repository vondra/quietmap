---
title: Costa Rica
intro: Roads use OSM class defaults scaled by 1.299. The Incofer suburban timetable is not loaded. Power plants from Global Energy Monitor.
map: { center: [-84.2, 9.9], zoom: 8 }
---

## Roads

No open traffic counts; roads use OSM class defaults. Motorways, trunks, primaries and their ramps are scaled by 1.299 (registered vehicles per road kilometre relative to Germany).

| OSM class | Default vehicles per day |
|---|---:|
| Motorway | 30,000 × 1.299 = 38,970 |
| Trunk | 15,000 × 1.299 = 19,485 |
| Primary | 9,000 × 1.299 = 11,691 |
| Secondary | 3,000 |
| Tertiary | 800 |
| Residential | 500 |
| Unclassified | 1,340 |
| Service | 250 |
| Track | 5 |

Where buildings are mapped, residential, living, service and unclassified streets are instead estimated from the buildings they serve.

## Railways

No timetable is loaded for the Incofer suburban trains around San José. Every line that OSM maps as a working railway, closed intercity lines included, takes the class default: 80 passenger and 20 freight trains per day on a main line, 30 and 5 on a branch.

## Industry

Power plants: Global Energy Monitor, operating plants only, where OSM maps an industrial area at the site. Other industry: OSM industrial areas with a type inferred from name and tags.

## Ships

Global Fishing Watch AIS vessel density.
