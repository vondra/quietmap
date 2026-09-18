---
title: United Arab Emirates
intro: Dubai Tram counts from the 2025 RTA timetable archive. Roads and Etihad Rail use class defaults; the Dubai Metro is not included.
map: { center: [54.5, 24.5], zoom: 7 }
---

## Roads

No per-road traffic counts are published. [Dubai Pulse](https://www.dubaipulse.gov.ae/) has none, and the Abu Dhabi portals are not accessible. Traffic is set by OpenStreetMap road class.

Motorways, trunk and primary roads use the world default × 1.300, from 831 vehicles per kilometre of road (Wikipedia fleet and road-length lists). The factor is limited to 0.7–1.3 and is at the cap here.

| Road class | Vehicles per day |
|---|---:|
| Motorway | 39,000 |
| Trunk | 19,500 |
| Primary | 11,700 |
| Secondary | 3,000 |
| Tertiary | 800 |
| Residential | 500 |

Local streets: traffic is derived from the buildings served; motorcycles 1%, from 0.4 × the two-wheeler share of registered vehicles (WHO 2023 country profile, 2021 fleet).

## Railways

Train counts come from the [Dubai RTA timetable on Dubai Pulse](https://www.dubaipulse.gov.ae/dataset/73765e8f-e8c4-443c-9687-288072ed9d12/resource/11515bd3-bdba-466f-ab65-f057bd123ab5/download/gtfs.7z). No current feed was available; the 2025 archive is used, with service up to 31 December 2025. Only the Dubai Tram is taken from it.

The Dubai Metro is tagged as subway in OpenStreetMap and is not included.

Etihad Rail publishes no timetable. Its lines use class defaults: 80 passenger and 20 freight trains per day on main lines, 30 and 5 on branches, 15 freight on industrial sidings.

## Industry

- Power plants: [Global Power Plant Database](https://datasets.wri.org/dataset/globalpowerplantdatabase), last updated in 2021; newer plants are missing.
- Steel works, cement plants, coal mines: Global Energy Monitor trackers.
- Refineries, smelters and the Jebel Ali port area: OSM polygons with a generic sound level.

## Ships

[Global Fishing Watch](https://globalfishingwatch.org/our-apis/) AIS data, which has no class for yachts and pleasure boats.
