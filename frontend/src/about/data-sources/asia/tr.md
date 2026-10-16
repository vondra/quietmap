---
title: Turkey
intro: Road traffic assigned by road class and city; train counts assigned per corridor. No counts or timetable are loaded.
---

## Roads

No open traffic counts were found from the highways directorate, KGM. Traffic is assigned by road class and city:

| Road class | Open country | 19 large cities (×1.4) | Ankara and İzmir (×1.8) | Istanbul (×2.5) |
|---|---:|---:|---:|---:|
| Motorway | 55,000 | 77,000 | 99,000 | 137,500 |
| Trunk | 22,000 | 30,800 | 39,600 | 55,000 |
| Primary | 12,000 | 16,800 | 21,600 | 30,000 |
| Secondary | 6,000 | 8,400 | 10,800 | 15,000 |
| Tertiary | 3,000 | 4,200 | 5,400 | 7,500 |
| Residential | 1,000 | 1,400 | 1,800 | 2,500 |

Each city is a box around its centre, not its administrative boundary. Residential streets take the table's value only where their buildings are not mapped; elsewhere they carry the trips their buildings make.

Vehicle mix: where not counted, a main road takes the medium and heavy shares of counted roads of its class, a street 1 % medium and 2 % heavy; motorcycles 2.8 % of a town street's traffic (WHO 2023); electric cars 1.3 % of cars, rolling noise only (IEA 2024).

Unsigned roads: a primary road 50 km/h in towns, 90 outside (cars drive 76 on a two-way one), a motorway 120.

## Railways

No Turkish timetable is loaded. Train counts are assigned per corridor:

| Track | Passenger | Freight |
|---|---:|---:|
| Marmaray, Istanbul (within 6 km) | 400 | 0 |
| İZBAN, İzmir (within 6 km) | 150 | 4 |
| High-speed lines from Ankara (within 8 km) | 40 | 0 |
| Ore lines to İskenderun and Zonguldak (within 12 km) | 6 | 20 |
| Main lines (within 12 km) | 20 | 12 |
| Other main-line track | 8 | 6 |
| Branches | 1 | 3 |
| Industrial sidings | 0 | 5 |

Tram track gets 350 trams per day in Istanbul and 250 elsewhere; light rail gets 500 and 400. Surface metro sections are included; see the [railway method](/about/data-preparation).

These heavy-rail counts are then scaled to the country's train-kilometres (Eurostat 2024): passenger trains by 0.372, freight by 0.681.

## Industry

- Power plants: Global Energy Monitor integrated power list, operating units only, wind farms excluded.
- Steel works, cement plants, coal mines: GEM trackers.
- Other factories: OSM polygons with a generic sound level.

## Ships

AIS data: EMODnet 2024 where its European grid reaches Turkish waters, [Global Fishing Watch](https://globalfishingwatch.org/our-apis/) elsewhere.

## Homes and places

Homes mapped in OpenStreetMap: 16 air conditioners per 100 homes (a world value). Mosques: five calls a day and the sala, on Friday by day and on Thursday evening.
