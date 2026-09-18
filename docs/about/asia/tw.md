---
title: Taiwan
intro: Roads and railways use class defaults; Freeway Bureau counts and TDX timetables are not loaded.
map: { center: [121.0, 23.7], zoom: 7 }
---

## Roads

The [Freeway Bureau](https://www.freeway.gov.tw/) publishes freeway traffic per section, but sections are named in text without coordinates, and the file is not joined to the map. Traffic is set by OpenStreetMap road class.

Motorways, trunk and primary roads use the world default × 1.299, from 546 vehicles per kilometre of road (Wikipedia fleet and road-length lists; factor limited to 0.7–1.3).
| Road class | Vehicles per day |
|---|---:|
| Motorway | 38,970 |
| Trunk | 19,485 |
| Primary | 11,691 |
| Secondary | 3,000 |
| Tertiary | 800 |
| Residential | 500 |

Local streets: traffic is derived from the buildings served; motorcycles 35%, set by hand because Taiwan has more registered scooters than cars and WHO has no profile for it.

## Railways

No timetable is loaded; the [TDX](https://tdx.transportdata.tw/) data hub has feeds for every operator but needs a registered account. All lines use class defaults: 80 passenger and 20 freight trains per day on main lines, 30 and 5 on branches, 15 freight on industrial sidings, 120 on tram lines, 80 on light rail. The high-speed line gets the main-line default.

The Taipei and Kaohsiung metros are tagged as subway in OpenStreetMap and are not included.

## Industry

- Power plants: [Global Power Plant Database](https://datasets.wri.org/dataset/globalpowerplantdatabase), last updated in 2021; newer plants are missing.
- Steel works, cement plants, coal mines: Global Energy Monitor trackers.
- Other factories: OSM polygons with a generic sound level.

## Ships

[Global Fishing Watch](https://globalfishingwatch.org/our-apis/) AIS data, which has no class for yachts and pleasure boats.
