---
title: South Korea
intro: Korean traffic counts and timetables are not accessible from abroad; roads and railways use class defaults.
map: { center: [127.8, 36.0], zoom: 6 }
---

## Roads

Korean traffic counts exist, but the government data portals refuse connections from outside the country and registration needs a Korean phone number. The same applies to the expressway API at [data.ex.co.kr](https://data.ex.co.kr/openapi/). Traffic is set by OpenStreetMap road class.

Motorways, trunk and primary roads use the world estimate per lane; smaller roads carry the trips their buildings make, else class defaults ([world defaults](/about/methodology)).

Local streets: traffic is derived from the buildings served; motorcycles 3%, set by hand because the Asia-wide value would be five times too high.

Vehicle mix: where not counted, the medium and heavy vehicles that counted roads of the same class carry; motorcycles 1.5 % of a town street's traffic (WHO 2023); electric cars 2.3 % of cars, rolling noise only (IEA 2024).

Unsigned roads: a primary road 50 km/h in towns, 60 outside, an expressway 100 (84 on a two-way one), a motorway 100.

## Railways

No open timetable is published for KORAIL or any metro. All lines use the [world railway defaults](/about/methodology).

Surface metro sections are included; see the [railway method](/about/methodology).

## Industry

- Power plants: [Global Power Plant Database](https://datasets.wri.org/dataset/globalpowerplantdatabase), last updated in 2021; newer plants are missing.
- Steel works, cement plants, coal mines: Global Energy Monitor trackers.

OSM industrial areas are also classified by Korean keywords in their names: 제철 or 철강 marks a steel works, 시멘트 a cement plant, and so on. Areas without a matching name stay generic. The Korean PRTR factory register is closed to foreign connections and is not used.

## Ships

[Global Fishing Watch](https://globalfishingwatch.org/our-apis/) AIS data, which has no class for yachts and pleasure boats.

## Homes and places

Homes mapped in OpenStreetMap: 86 air conditioners per 100 homes. Mosques: only those with a mapped minaret call, once on Fridays.
