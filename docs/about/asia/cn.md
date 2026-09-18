---
title: China
intro: Mainland China. Road traffic and train counts are assigned by road class and line speed from community network maps.
map: { center: [105.0, 35.0], zoom: 4 }
---

## Roads

No open traffic counts. A community map of the national road network on [ArcGIS Online](https://services1.arcgis.com/ERdCHt0sNM6dENSD/) has three classes. OpenStreetMap motorways, trunk and primary roads within 400 m of a network line get traffic by that class:

| Network class | Open country | 33 large cities (×1.4) | 12 largest cities (×2.0) |
|---|---:|---:|---:|
| Highway | 60,000 | 84,000 | 120,000 |
| Major road | 25,000 | 35,000 | 50,000 |
| Local road | 8,000 | 11,200 | 16,000 |

The 12 largest cities are Beijing, Shanghai, Guangzhou, Shenzhen, Chengdu, Chongqing, Wuhan, Xi'an, Hangzhou, Nanjing, Suzhou and Tianjin. Each city is a bounding box, not its administrative boundary.

Vehicle split in cities: 75% cars, 10% medium, 10% heavy, 5% motorcycles. Outside cities: 65, 12, 18 and 5%. The motorcycle share is low because most large cities ban petrol motorcycles and electric scooters make almost no engine noise.

None of these values is a count. Main roads with no network line nearby get the world default × 1.017: motorway 30,510, trunk 15,255, primary 9,153.

## Railways

No operator publishes a timetable. A community map of the mainland network on [ArcGIS Online](https://services7.arcgis.com/m6uLpqj7MgjPU371/) gives each national line a top speed and each metro line a service type; lines marked as not operating are skipped. Track within 500 m of a mapped line gets trains by speed:

| Top speed | Passenger | Freight |
|---|---:|---:|
| 350 km/h | 180 | 0 |
| 300 km/h | 150 | 0 |
| 250 km/h | 120 | 0 |
| 200 km/h | 80 | 10 |
| 150 km/h | 50 | 20 |
| 100 km/h | 30 | 20 |
| Slower | 15 | 10 |

Metro lines get 500 trains per day, express metro 400, light rail 300, streetcars 200, assigned by line type.

Metro lines tagged as ordinary rail in OpenStreetMap are on the map. Lines tagged as subway are not included.

## Industry

- Coal, gas and nuclear plants, LNG terminals and solar farms: Global Energy Monitor lists, operating units only, matched to OpenStreetMap industrial areas within 1.5 km. Wind farms are not used.
- Steel works, cement plants, coal mines: GEM trackers.
- Other factories: OSM polygons with a generic sound level.

## Ships

[Global Fishing Watch](https://globalfishingwatch.org/our-apis/) AIS data, which has no class for yachts and pleasure boats.
