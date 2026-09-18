---
title: Saudi Arabia
intro: Ministry of Transport station counts 2024 on numbered roads, assigned values elsewhere. No railway timetable.
map: { center: [45.0, 24.0], zoom: 6 }
---

## Roads

Three sources, applied in this order.

Numbered roads: [Ministry of Transport](https://mot.gov.sa/en/open-data) count stations, 24-hour totals for 2024. Every OpenStreetMap road with a given number gets the average of the stations on that road. The stations stand on open desert sections, so the single value per road is too low inside Riyadh and Jeddah.

Riyadh: the city's pavement management map gives a street class (A to D) and a lane count, but no counts. Traffic is assigned from class and lanes:

| Riyadh street class | Vehicles per day |
|---|---:|
| A, 5 lanes or more | 50,000 |
| A, 4 lanes | 35,000 |
| A, fewer | 22,000 |
| B | 8,000 to 18,000 |
| C | 3,500 to 6,000 |
| D | 900 to 1,800 |

Elsewhere: roads are matched to the national [transport atlas](https://www.arcgis.com/home/item.html?id=a69a52e770ba4f91950cfd208c556dcb) within 250 m. A primary route gets 6,000 vehicles per day, a secondary route 1,500, anything else 800. Most Saudi main roads fall into this group.

All three use one vehicle split: 78% cars, 10% medium, 11% heavy, 1% motorcycles. Residential streets are not covered; their traffic is derived from the buildings served.

## Railways

No Saudi operator publishes a timetable feed. All lines use class defaults: 80 passenger and 20 freight trains per day on main lines, 30 and 5 on branches, 15 freight on industrial sidings, 80 on light rail. The Haramain high-speed line and the northern freight line both get the main-line default. The Riyadh Metro is included, at 80 trains per day, where OpenStreetMap tags it as light rail.

## Industry

- Power plants: [Global Power Plant Database](https://datasets.wri.org/dataset/globalpowerplantdatabase), last updated in 2021; newer plants are missing.
- Steel works, cement plants, coal mines: Global Energy Monitor trackers.
- Refineries and petrochemical plants at Jubail, Yanbu and Ras Tanura: OSM polygons with a generic sound level.

## Ships

[Global Fishing Watch](https://globalfishingwatch.org/our-apis/) AIS data, which has no class for yachts and pleasure boats.
