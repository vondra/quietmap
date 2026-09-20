---
title: Thailand
intro: Department of Rural Roads counts 2024 on rural roads, assigned values on highways. Passenger train counts from the national Namtang timetable.
map: { center: [100.5, 14.0], zoom: 6 }
---

## Roads

Rural roads: Department of Rural Roads daily traffic for 2024, per road and in twelve vehicle classes including motorcycles, from the [Ministry of Transport data portal](https://datagov.mot.go.th/). Counts are matched to OpenStreetMap by the exact Thai road code, such as นบ.3021; the published vehicle split is kept.

Motorways and national highways have no usable open counts: the accessible Department of Highways file reports vehicle-kilometres per year, not vehicles per section. Values are assigned per route number:

| Route | Vehicles per day | In Bangkok |
|---|---:|---:|
| Motorway 7 | 120,000 | |
| Motorway 9 | 100,000 | |
| Highway 35 | 90,000 | 130,000 |
| Highway 34 | 80,000 | 120,000 |
| Highway 32 | 45,000 | 85,000 |
| Highway 1 | 35,000 | 95,000 |
| Highway 2 | 30,000 | 85,000 |
| Highway 4 | 28,000 | 80,000 |
| Highway 3 | 25,000 | 75,000 |

Nine more routes (11, 12, 22, 24, 33, 41, 81, 82, 304) have their own values between 15,000 and 50,000.

All other roads use a Thai class default, higher than the world default and with more motorcycles:

| Road class | Thailand | Bangkok |
|---|---:|---:|
| Motorway | 60,000 | 90,000 |
| Trunk | 30,000 | 45,000 |
| Primary | 15,000 | 22,500 |
| Secondary | 6,000 | 9,000 |
| Tertiary | 2,500 | 3,750 |
| Residential | 1,200 | 1,800 |

Vehicle split on roads with assigned values: 62% cars, 10% medium, 13% heavy, 15% motorcycles; in Bangkok 60, 8, 7 and 25%.

## Railways

Train counts come from the national [Namtang timetable](https://namtang-api.otp.go.th/download/namtang-gtfs.zip) of the Office of Transport and Traffic Policy and Planning, busiest Wednesday. It covers the State Railway, the BTS Skytrain and the Airport Rail Link; freight is not included.

Surface metro sections are included; see the [railway method](/about/methodology).

## Industry

- Power plants: [Global Power Plant Database](https://datasets.wri.org/dataset/globalpowerplantdatabase), last updated in 2021; newer plants are missing.
- Steel works, cement plants, coal mines: Global Energy Monitor trackers.
- Other industrial areas, including Map Ta Phut and Laem Chabang: OSM polygons with a generic sound level. The Thai factory register needs a login and is not used.

## Ships

[Global Fishing Watch](https://globalfishingwatch.org/our-apis/) AIS data, which has no class for yachts and pleasure boats.
