---
title: Indonesia
intro: Bina Marga daily traffic (LHRT) on many regional roads, assigned values elsewhere. No railway timetable.
map: { center: [118.0, -2.0], zoom: 4 }
---

## Roads

The highways directorate, Bina Marga, runs a public [GIS portal](https://gisportal.binamarga.pu.go.id/) with three layers. They are applied to motorways, trunk and primary roads in this order:

- Toll roads: a road within 300 m of an operating toll road gets 80,000 vehicles per day, an assigned value.
- Regional roads: the layer carries LHRT, the average daily traffic, for many provincial, regency and city roads. A road within 200 m of such a line gets the published value. Where the value is empty, the road gets 12,000 (city road), 8,000 (provincial) or 5,000 (regency).
- National roads: no traffic value. A road within 400 m gets 30,000.

Assigned values are doubled in 8 metropolitan areas (Jakarta, Surabaya, Bandung, Medan, Semarang, Makassar, Palembang, Denpasar) and multiplied by 1.4 in 33 other cities. Published LHRT values are used unchanged.

| Where | Cars | Medium | Heavy | Motorcycles |
|---|---:|---:|---:|---:|
| 8 metropolitan areas | 30% | 5% | 5% | 60% |
| 33 other cities | 40% | 6% | 4% | 50% |
| Open country | 50% | 8% | 7% | 35% |

The vehicle split is assigned; LHRT is a single total without vehicle classes. Smaller roads use the world default. Local-street traffic is derived from the buildings served, with 33% motorcycles.

## Railways

No Indonesian operator publishes an open timetable. All lines use class defaults: 80 passenger and 20 freight trains per day on main lines, 30 and 5 on branches, 15 freight on industrial sidings, 120 on tram lines, 80 on light rail. Underground sections of the Jakarta MRT are tagged as subway in OpenStreetMap and are not included.

## Industry

- Power plants: Global Energy Monitor list for Indonesia, operating units only, wind farms excluded, matched to OpenStreetMap industrial areas within 1.5 km.
- Steel works, cement plants, coal mines: GEM trackers.
- Palm oil mills, nickel smelters and refineries: OSM polygons with a generic sound level.

## Ships

[Global Fishing Watch](https://globalfishingwatch.org/our-apis/) AIS data, which has no class for yachts and pleasure boats.
