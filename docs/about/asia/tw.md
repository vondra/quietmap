---
title: Taiwan
intro: Roads and railways use class defaults; Freeway Bureau counts and TDX timetables are not loaded.
map: { center: [121.0, 23.7], zoom: 7 }
---

## Roads

The [Freeway Bureau](https://www.freeway.gov.tw/) publishes freeway traffic per section, but sections are named in text without coordinates, and the file is not joined to the map. Traffic is set by OpenStreetMap road class.

Motorways, trunk and primary roads use the world estimate per lane; smaller roads use class defaults ([world defaults](/about/methodology)).

Local streets: traffic is derived from the buildings served; motorcycles 35%, set by hand because Taiwan has more registered scooters than cars and WHO has no profile for it.

## Railways

No timetable is loaded; the [TDX](https://tdx.transportdata.tw/) data hub has feeds for every operator but needs a registered account. All lines use the [world railway defaults](/about/methodology). The high-speed line gets the main-line default.

Surface metro sections are included; see the [railway method](/about/methodology).

## Industry

- Power plants: [Global Power Plant Database](https://datasets.wri.org/dataset/globalpowerplantdatabase), last updated in 2021; newer plants are missing.
- Steel works, cement plants, coal mines: Global Energy Monitor trackers.
- Other factories: OSM polygons with a generic sound level.

## Ships

[Global Fishing Watch](https://globalfishingwatch.org/our-apis/) AIS data, which has no class for yachts and pleasure boats.
