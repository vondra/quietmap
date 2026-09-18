---
title: India
intro: Road traffic by class of the official Bharatmala network. Train counts from a community timetable and the official network map.
map: { center: [78.0, 22.0], zoom: 5 }
---

## Roads

No open traffic counts; the highway authority's census is PDF only. The official Bharatmala road network on [Esri Living Atlas India](https://livingatlas.esri.in/) gives a class per road. OpenStreetMap motorways, trunk and primary roads within 300 m of a network line get traffic by that class:

| Network class | Open country | 33 large cities (×1.3) | 8 largest cities (×2.0) |
|---|---:|---:|---:|
| Expressway | 80,000 | 104,000 | 160,000 |
| Ring road | 50,000 | 65,000 | 100,000 |
| National highway | 35,000 | 45,500 | 70,000 |
| State highway | 15,000 | 19,500 | 30,000 |

The 8 largest cities are Delhi, Mumbai, Bangalore, Hyderabad, Chennai, Kolkata, Ahmedabad and Pune.

| Where | Cars | Medium | Heavy | Motorcycles |
|---|---:|---:|---:|---:|
| 8 largest cities | 45% | 8% | 7% | 40% |
| 33 large cities | 50% | 9% | 10% | 31% |
| Open country | 55% | 10% | 15% | 20% |

None of these values is a count. Main roads with no network line nearby get the world default × 1.042: motorway 31,260, trunk 15,630, primary 9,378. On local streets motorcycles are 30% of traffic.

## Railways

Lines in the [community-built Indian Railways timetable](https://github.com/Neo2308/indianrailways-gtfs) get its passenger train counts. The timetable is unofficial and has no freight.

Other lines use the official railway network map on [Esri Living Atlas India](https://livingatlas.esri.in/), which gives speed, gauge and railway zone. Track within 500 m of a mapped line gets trains by location and speed:

| Line | Passenger | Freight |
|---|---:|---:|
| Mumbai suburban (Central and Western zones) | 1,300 | 30 |
| Kolkata suburban | 500 | 25 |
| Delhi and Chennai suburban | 350 | 20 |
| Bangalore, Hyderabad, Ahmedabad, Pune | 60 | 20 |
| 120 km/h or faster | 30 | 15 |
| 100 to 119 km/h | 25 | 15 |
| 60 to 99 km/h | 15 | 10 |
| Slower | 8 | 5 |
| Metre and narrow gauge | 5 | 0 |

Metro lines on the map get 400 trains per day. Lines tagged as subway in OpenStreetMap are not included.

## Industry

- Cement plants, power plants and industrial parks: Living Atlas India, matched to OpenStreetMap industrial areas within 1 km.
- Parks are typed by their Central Pollution Control Board category: red is treated as heavy metal industry, orange as chemicals, green as textiles, white as offices, no category as metal products.
- Steel works and coal mines: Global Energy Monitor trackers.
- Other factories: OSM polygons with a generic sound level.

## Ships

[Global Fishing Watch](https://globalfishingwatch.org/our-apis/) AIS data, which has no class for yachts and pleasure boats.
