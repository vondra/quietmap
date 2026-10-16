---
title: Israel
intro: Train counts from the Ministry of Transport national timetable, passenger only. Main roads use the world defaults; smaller roads are estimated from the buildings around them.
---

## Roads

No usable per-road traffic counts. The national road company's site blocks automated access, and the open survey file on data.gov.il contains only scattered 15-minute counts.

Motorways, trunk and primary roads use the world estimate per lane; secondary and tertiary roads follow the trips made by the buildings within 5 km, streets the trips their buildings make, else class defaults ([world defaults](/about/data-preparation)).

Vehicle mix: where not counted, a main road takes the medium and heavy shares of counted roads of its class, a street 1 % medium and 2 % heavy; motorcycles 0.7 % of a town street's traffic (WHO 2023); electric cars 4.2 % of cars, rolling noise only (IEA 2024).

Unsigned roads: a primary road 50 km/h in towns, 80 outside (cars drive 68 on a two-way one), a motorway 110.

## Railways

Train counts come from the [Ministry of Transport's national timetable](https://www.gov.il/he/pages/gtfs_general_transit_feed_specifications), which covers every public transport operator. Israel Railways and the Tel Aviv and Jerusalem light rail are used, counted on the busiest Wednesday. Freight is not in the timetable and takes the class defaults.

## Industry

- Power plants: [Global Power Plant Database](https://datasets.wri.org/dataset/globalpowerplantdatabase), last updated in 2021; newer plants are missing.
- Steel works, cement plants, coal mines: Global Energy Monitor trackers.
- Other factories: OSM polygons with a generic sound level.

## Ships

AIS data: EMODnet 2024 where its European grid reaches, [Global Fishing Watch](https://globalfishingwatch.org/our-apis/) elsewhere.

## Homes and places

Homes mapped in OpenStreetMap: 16 air conditioners per 100 homes (a world value). Mosques call five times a day.
