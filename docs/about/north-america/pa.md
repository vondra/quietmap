---
title: Panama
intro: Roads use OSM class defaults scaled by 0.995, a factor derived from population density. The Panama Canal Railway uses the class default; the Panama Metro is not included. Power plants from Global Energy Monitor.
map: { center: [-80.1, 8.6], zoom: 8 }
---

## Roads

No open traffic counts; roads use OSM class defaults. Motorways, trunks, primaries and their ramps are scaled by 0.995. No vehicles-per-kilometre figure is available for Panama, so the factor is derived from population density, a weaker proxy.

| OSM class | Default vehicles per day |
|---|---:|
| Motorway | 30,000 × 0.995 = 29,850 |
| Trunk | 15,000 × 0.995 = 14,925 |
| Primary | 9,000 × 0.995 = 8,955 |
| Secondary | 3,000 |
| Tertiary | 800 |
| Residential | 500 |
| Unclassified | 1,340 |
| Service | 250 |
| Track | 5 |

Where buildings are mapped, residential, living, service and unclassified streets are instead estimated from the buildings they serve.

## Railways

The Panama Canal Railway (Panama City to Colón) publishes no timetable and takes the class default of 80 passenger and 20 freight trains per day. The Panama Metro is tagged railway=subway in OSM and is not included.

## Industry

Power plants: Global Energy Monitor, operating plants only, where OSM maps an industrial area at the site. Other industry: OSM industrial areas with a type inferred from name and tags.

## Ships

Global Fishing Watch AIS vessel density.
