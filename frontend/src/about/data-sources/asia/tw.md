---
title: Taiwan
intro: Main roads and railways use class defaults, smaller roads are estimated from the buildings around them; Freeway Bureau counts and TDX timetables are not loaded.
---

## Roads

The [Freeway Bureau](https://www.freeway.gov.tw/) publishes freeway traffic per section, but sections are named in text without coordinates, and the file is not joined to the map.

Motorways, trunk and primary roads use the world estimate per lane; secondary and tertiary roads follow the trips made by the buildings within 5 km, streets the trips their buildings make, else class defaults ([world defaults](/about/data-preparation)).

Vehicle mix: where not counted, a main road takes the medium and heavy shares of counted roads of its class, a street 1 % medium and 2 % heavy; motorcycles 35 % of a town street's traffic (set by hand: Taiwan registers more scooters than cars and WHO has no profile for it).

Unsigned roads: a primary road 60 km/h in towns, 50 outside.

## Railways

No timetable is loaded; the [TDX](https://tdx.transportdata.tw/) data hub has feeds for every operator but needs a registered account. All lines use the [world railway defaults](/about/data-preparation). The high-speed line gets the main-line default.

Surface metro sections are included; see the [railway method](/about/data-preparation).

## Industry

- Power plants: [Global Power Plant Database](https://datasets.wri.org/dataset/globalpowerplantdatabase), last updated in 2021; newer plants are missing.
- Steel works, cement plants, coal mines: Global Energy Monitor trackers.
- Other factories: OSM polygons with a generic sound level.

## Ships

[Global Fishing Watch](https://globalfishingwatch.org/our-apis/) AIS data, which has no class for yachts and pleasure boats.

## Homes and places

Homes mapped in OpenStreetMap: 146 air conditioners per 100 homes (the region's median). Mosques: only those with a mapped minaret call, once on Fridays.
