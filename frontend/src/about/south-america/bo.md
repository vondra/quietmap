---
title: Bolivia
intro: No traffic counts; main routes are estimated from road surface. No train timetable is loaded. Power plants and substations from the energy ministry.
map: { center: [-65, -17], zoom: 5 }
---

## Roads

The road agency ABC publishes no machine-readable traffic counts, and the national geodata portal closed in 2023. The primary road network comes from two community mirrors: the ABC Red Vial Fundamental 2024, which covers part of the country, and a wider layer of first-order routes. Both give route location and surface.

An OSM motorway, trunk or primary road within 500 m of one of those routes is estimated from the surface:

| Surface | Vehicles per day |
|---|---:|
| Paved | 15,000 |
| Urban section | 12,000 |
| Gravel, or paving under construction | 5,000 |
| Dirt | 2,000 |
| Not stated | 6,000 |

The estimate is doubled inside La Paz and El Alto, Santa Cruz de la Sierra and Cochabamba, and multiplied by 1.4 in 16 other towns: Sucre, Oruro, Tarija, Potosí, Trinidad, Cobija, Riberalta, Montero, Quillacollo, Sacaba, Warnes, Yacuiba, Camiri, Villazón, Viacha and Uyuni. No measurements are available to validate these values.

The vehicle mix is an estimate:

Vehicle mix: where not counted, the medium and heavy vehicles that counted roads of the same class carry; motorcycles 9.7 % of a town street's traffic (WHO 2023).

Altiplano, valleys and lowlands are divided at 67° W and 65° W. The mining corridors are two boxes around Oruro, Potosí and Uyuni.

Motorways, trunks and primaries not near a mapped main route use the world estimate per lane. Secondary and smaller roads use class defaults ([world defaults](/about/methodology)).

Unsigned roads: a primary road 40 km/h in towns, 60 outside, a residential street 20.

## Railways

No timetable is loaded; neither the western nor the eastern network publishes a schedule. All lines use the [world railway defaults](/about/methodology).

## Industry

Power plants and substations: open map layers of the Ministerio de Hidrocarburos y Energía, covering plants of the interconnected system, plants of the isolated systems in Beni and Pando, and substations of 69 kV and above. Plants missing there come from the Global Energy Monitor power tracker. Each record is attached to an OSM industrial area within 2 km.

Mines, smelters, gas fields and refineries are in no loadable register; they are OSM industrial areas with a type inferred from name and tags.

## Homes and places

Homes mapped in OpenStreetMap: 16 air conditioners per 100 homes (the region's median). Mosques: only those with a mapped minaret call, once on Fridays.
