---
title: Israel
intro: Train counts from the Ministry of Transport national timetable, passenger only. Roads use the world defaults.
map: { center: [35.0, 31.5], zoom: 7 }
---

## Roads

No usable per-road traffic counts. The national road company's site blocks automated access, and the open survey file on data.gov.il contains only scattered 15-minute counts. Traffic is set by OpenStreetMap road class.

Motorways, trunk and primary roads use the world estimate per lane; smaller roads use class defaults ([world defaults](/about/methodology)).

Local streets: traffic is derived from the buildings served; motorcycles 2%, from 0.4 × the two-wheeler share of registered vehicles (WHO 2023 country profile, 2021 fleet).

## Railways

Train counts come from the [Ministry of Transport's national timetable](https://www.gov.il/he/pages/gtfs_general_transit_feed_specifications), which covers every public transport operator. Israel Railways and the Tel Aviv and Jerusalem light rail are used, counted on the busiest Wednesday. Freight is not in the timetable.

## Industry

- Power plants: [Global Power Plant Database](https://datasets.wri.org/dataset/globalpowerplantdatabase), last updated in 2021; newer plants are missing.
- Steel works, cement plants, coal mines: Global Energy Monitor trackers.
- Other factories: OSM polygons with a generic sound level.

## Ships

AIS data: EMODnet 2024 where its European grid reaches, [Global Fishing Watch](https://globalfishingwatch.org/our-apis/) elsewhere.
