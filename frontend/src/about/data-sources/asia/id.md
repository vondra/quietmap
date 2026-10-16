---
title: Indonesia
intro: Bina Marga daily traffic (LHRT) on some provincial roads, assigned values on most other main roads. No railway timetable.
---

## Roads

The highways directorate, Bina Marga, runs a public [GIS portal](https://gisportal.binamarga.pu.go.id/) with three layers. They are applied to motorways, trunk and primary roads in this order:

- Toll roads: a road within 300 m of an operating toll road gets 80,000 vehicles per day, an assigned value.
- Regional roads: the layer carries LHRT, the average daily traffic, for many provincial, regency and city roads. A primary road within 200 m of a provincial line gets the published value, or 8,000 where the value is empty, unless a national road lies within 50 m: most regency and city lines run beside national roads, so their counts are not used on main roads.
- National roads: no traffic value. A road within 400 m gets 30,000.

Assigned values are doubled in 8 metropolitan areas (Jakarta, Surabaya, Bandung, Medan, Semarang, Makassar, Palembang, Denpasar) and multiplied by 1.4 in 33 other cities. Published LHRT values are used unchanged.

Vehicle mix: where not counted, a main road takes the medium and heavy shares of counted roads of its class, a street 1 % medium and 2 % heavy; motorcycles 64 % of a town street's traffic (WHO 2023); electric cars 0.6 % of cars, rolling noise only (IEA 2024).

LHRT is a single total without vehicle classes; its roads take an assigned split: 35 % motorcycles, 8 % medium and 7 % heavy vehicles in open country, 50, 6 and 4 % in the 33 cities, 60, 5 and 5 % in the 8 metropolitan areas. Main roads none of the layers reach, 28 % of their length, use the world estimate per lane ([world defaults](/about/data-preparation)); secondary and tertiary roads follow the trips made by the buildings within 5 km, streets the trips their buildings make.

Unsigned roads: a primary road 50 km/h in towns, 60 outside, a motorway 100.

## Railways

No Indonesian operator publishes an open timetable. All lines use the [world railway defaults](/about/data-preparation). Surface metro sections are included; see the [railway method](/about/data-preparation).

## Industry

- Power plants: Global Energy Monitor list for Indonesia, operating units only, wind farms excluded, matched to OpenStreetMap industrial areas within 1.5 km.
- Steel works, cement plants, coal mines: GEM trackers.
- Palm oil mills, nickel smelters and refineries: OSM polygons with a generic sound level.

## Ships

[Global Fishing Watch](https://globalfishingwatch.org/our-apis/) AIS data, which has no class for yachts and pleasure boats.

## Homes and places

Homes mapped in OpenStreetMap: 9 air conditioners per 100 homes. Mosques: five calls a day with the recitation before them. Open-fronted bars and restaurants, their terraces open as long as they are.
