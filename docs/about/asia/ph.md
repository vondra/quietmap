---
title: Philippines
intro: Road traffic by class of the official DPWH road network. No railway timetable; all lines use class defaults.
map: { center: [122.0, 12.0], zoom: 6 }
---

## Roads

No open traffic counts. The Department of Public Works and Highways publishes its [national road network](https://services1.arcgis.com/IwZZTMxZCmAmFYvF/) with a class per section. OpenStreetMap motorways, trunk and primary roads within 300 m of a section get traffic by that class:

| DPWH class | Open country | 19 cities (×1.4) | Metro Manila (×2.0) |
|---|---:|---:|---:|
| Primary | 50,000 | 70,000 | 100,000 |
| Secondary | 20,000 | 28,000 | 40,000 |
| Tertiary | 8,000 | 11,200 | 16,000 |

| Where | Cars | Medium | Heavy | Motorcycles |
|---|---:|---:|---:|---:|
| Metro Manila | 35% | 8% | 7% | 50% |
| 19 cities | 45% | 8% | 7% | 40% |
| Open country | 55% | 10% | 10% | 25% |

None of these values is a count. Main roads with no DPWH section nearby get the world estimate per lane ([world defaults](/about/methodology)).

## Railways

No timetable is loaded. All lines use the [world railway defaults](/about/methodology). This includes LRT-1, LRT-2 and MRT-3 in Manila where OpenStreetMap tags them as light rail.

## Industry

- Power plants: Global Energy Monitor list for the Philippines, operating units only.
- Economic zones: from a zone map, treated as general manufacturing.
- Both are matched to OpenStreetMap industrial areas within 1.5 km; other factories are OSM polygons with a generic sound level.

## Ships

[Global Fishing Watch](https://globalfishingwatch.org/our-apis/) AIS data, which has no class for yachts and pleasure boats.
