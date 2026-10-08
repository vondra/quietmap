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

Vehicle mix: where not counted, the medium and heavy vehicles that counted roads of the same class carry; motorcycles 2 % of a town street's traffic (a local estimate); electric cars 0.5 % of cars, rolling noise only (IEA 2024); lorries and buses at most 90 km/h.

Unsigned roads: a primary road 40 km/h in towns, 50 outside, a motorway 100.

## Railways

No Japanese timetable is loaded; the [ODPT](https://developer.odpt.org/) feeds for Tokyo need a registered key. All lines, the Shinkansen included, use the [world railway defaults](/about/methodology).

Surface metro sections are included; see the [railway method](/about/methodology).

## Industry

- Power plants: [Global Power Plant Database](https://datasets.wri.org/dataset/globalpowerplantdatabase), last updated in 2021; newer plants are missing.
- Steel works, cement plants, coal mines: Global Energy Monitor trackers.
- Other industrial areas: OSM polygons with a generic sound level. The Japanese PRTR factory register is not used.

## Ships

[Global Fishing Watch](https://globalfishingwatch.org/our-apis/) AIS data, which has no class for yachts and pleasure boats.

## Homes and places

Homes mapped in OpenStreetMap: 237 air conditioners per 100 homes. Mosques: only those with a mapped minaret call, once on Fridays.
