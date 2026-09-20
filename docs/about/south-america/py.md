---
title: Paraguay
intro: No traffic counts; the national routes in the MOPC 2023 network are estimated from their surface. No train timetable is loaded. Power plants from Global Energy Monitor, including Itaipú and Yacyretá.
map: { center: [-57.5, -23.5], zoom: 6 }
---

## Roads

Road network: the national routes file of the public works ministry MOPC, linked from its [road network page](https://www.mopc.gov.py/red-vial/), 2023 edition. It gives routes and surface but no traffic; no machine-readable counts are published in Paraguay.

An OSM motorway, trunk or primary road within 500 m of a national route is estimated from the surface:

| Surface | Vehicles per day |
|---|---:|
| Paved | 12,000 |
| Part paved, part dirt | 8,000 |
| Dirt | 3,000 |
| Not stated | 6,000 |

The estimate is doubled inside Greater Asunción and multiplied by 1.4 in 17 other towns, among them Ciudad del Este, Encarnación, Pedro Juan Caballero, Concepción, Coronel Oviedo, Villarrica, Pilar, Caaguazú, Filadelfia and Salto del Guairá.

The vehicle mix is an estimate:

| Where | Light | Medium | Heavy | Motorcycle |
|---|---:|---:|---:|---:|
| Greater Asunción | 65% | 5% | 10% | 20% |
| The 17 towns | 62% | 6% | 12% | 20% |
| Chaco | 60% | 10% | 20% | 10% |
| Eastern region | 55% | 8% | 25% | 12% |

Motorways, trunks and primaries that are not a national route use the world estimate per lane. Secondary and smaller roads use class defaults ([world defaults](/about/methodology)).

## Railways

No timetable is loaded. Any stretch that OSM still maps as a working railway uses the [world railway defaults](/about/methodology); mapped disused or abandoned lines are excluded.

## Industry

Power plants: Global Energy Monitor power tracker, covering plants in Paraguay and the binational dams Itaipú and Yacyretá. Each is attached to an OSM industrial area within 3 km. Other industry: OSM industrial areas with a type inferred from name and tags.

## Ships

River traffic on the Paraguay and Paraná appears only where Global Fishing Watch AIS vessel density covers it.
