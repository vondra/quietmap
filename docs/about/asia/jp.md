---
title: Japan
intro: MLIT road traffic census 2021 on main roads. No railway timetable; all lines use class defaults.
map: { center: [137.0, 36.5], zoom: 5 }
---

## Roads

Traffic on main roads comes from the [MLIT road traffic census of 2021](https://www.mlit.go.jp/road/census/r3/), all 47 prefectures: 24-hour counts of small and large vehicles per surveyed section.

The census has no open geometry; section numbers resolve to a location only through the paid Digital Road Map. Counts are therefore joined by identity: expressways by name, national highways by route number. Each matched road gets the median of all counted sections on its route, so one value applies along the whole route.

Unmatched roads and prefectural roads get the census median for their road class.

Large vehicles are split 25% medium and 75% heavy, as the census gives no axle split. Motorcycles are not counted in the census and are set to zero on census roads. On local streets, where traffic is derived from the buildings served, motorcycles are 2%.

## Railways

No Japanese timetable is loaded; the [ODPT](https://developer.odpt.org/) feeds for Tokyo need a registered key. All lines, the Shinkansen included, use class defaults: 80 passenger and 20 freight trains per day on main lines, 30 and 5 on branches, 15 freight on industrial sidings, 120 on tram lines, 80 on light rail.

Lines tagged as subway in OpenStreetMap are not included. Tokyo Metro, Toei and the Osaka Metro are missing, including their above-ground sections.

## Industry

- Power plants: [Global Power Plant Database](https://datasets.wri.org/dataset/globalpowerplantdatabase), last updated in 2021; newer plants are missing.
- Steel works, cement plants, coal mines: Global Energy Monitor trackers.
- Other industrial areas: OSM polygons with a generic sound level. The Japanese PRTR factory register is not used.

## Ships

[Global Fishing Watch](https://globalfishingwatch.org/our-apis/) AIS data, which has no class for yachts and pleasure boats.
