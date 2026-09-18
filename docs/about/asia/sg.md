---
title: Singapore
intro: Road traffic by OpenStreetMap class, main roads scaled by 1.292. The MRT is not included; the LRT loops use the light-rail default.
map: { center: [103.85, 1.35], zoom: 11 }
---

## Roads

No per-road traffic counts are published. [data.gov.sg](https://data.gov.sg/) has one annual figure, vehicles entering the city; the LTA DataMall speed data needs an API key. Traffic is set by OpenStreetMap road class.

Motorways, trunk and primary roads use the world default × 1.292, from 285 vehicles per kilometre of road (Wikipedia fleet and road-length lists; factor limited to 0.7–1.3).
| Road class | Vehicles per day |
|---|---:|
| Motorway | 38,760 |
| Trunk | 19,380 |
| Primary | 11,628 |
| Secondary | 3,000 |
| Tertiary | 800 |
| Residential | 500 |

Local streets: traffic is derived from the buildings served; motorcycles 15% (Asia-wide value, calibrated on Thailand, India and Vietnam).

## Railways

The MRT lines are tagged as subway in OpenStreetMap and are not included. The map mostly shows the LRT loops at Bukit Panjang, Sengkang and Punggol, at the class default of 80 trains per day. The LTA timetable needs a registered key and is not loaded.

## Industry

- Power plants: [Global Power Plant Database](https://datasets.wri.org/dataset/globalpowerplantdatabase), last updated in 2021; newer plants are missing.
- Steel works, cement plants, coal mines: Global Energy Monitor trackers.
- Jurong Island and the port terminals: OSM polygons with a generic sound level.

## Ships

[Global Fishing Watch](https://globalfishingwatch.org/our-apis/) AIS data, which has no class for yachts and pleasure boats.
