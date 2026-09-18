---
title: South Korea
intro: Korean traffic counts and timetables are not accessible from abroad; roads and railways use class defaults.
map: { center: [127.8, 36.0], zoom: 6 }
---

## Roads

Korean traffic counts exist, but the government data portals refuse connections from outside the country and registration needs a Korean phone number. The same applies to the expressway API at [data.ex.co.kr](https://data.ex.co.kr/openapi/). Traffic is set by OpenStreetMap road class.

Motorways, trunk and primary roads use the world default × 1.292, from 280 vehicles per kilometre of road (Wikipedia fleet and road-length lists; factor limited to 0.7–1.3).
| Road class | Vehicles per day |
|---|---:|
| Motorway | 38,760 |
| Trunk | 19,380 |
| Primary | 11,628 |
| Secondary | 3,000 |
| Tertiary | 800 |
| Residential | 500 |

Local streets: traffic is derived from the buildings served; motorcycles 3%, set by hand because the Asia-wide value would be five times too high.

## Railways

No open timetable is published for KORAIL or any metro. All lines use class defaults: 80 passenger and 20 freight trains per day on main lines, 30 and 5 on branches, 15 freight on industrial sidings, 120 on tram lines, 80 on light rail.

Lines tagged as subway in OpenStreetMap are not included. This covers the Seoul, Busan, Daegu, Daejeon, Gwangju and Incheon metros.

## Industry

- Power plants: [Global Power Plant Database](https://datasets.wri.org/dataset/globalpowerplantdatabase), last updated in 2021; newer plants are missing.
- Steel works, cement plants, coal mines: Global Energy Monitor trackers.

OSM industrial areas are also classified by Korean keywords in their names: 제철 or 철강 marks a steel works, 시멘트 a cement plant, and so on. Areas without a matching name stay generic. The Korean PRTR factory register is closed to foreign connections and is not used.

## Ships

[Global Fishing Watch](https://globalfishingwatch.org/our-apis/) AIS data, which has no class for yachts and pleasure boats.
