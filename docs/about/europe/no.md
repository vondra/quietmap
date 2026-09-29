---
title: Norway
intro: Traffic volumes on state and county roads (NVDB), national passenger timetable, wind farm ratings from the NVE register. Municipal streets and rail freight not covered.
map: { center: [10.0, 64.5], zoom: 5 }
---

## Roads

State and county roads: [NVDB](https://nvdbapiles.atlas.vegvesen.no/), the national road database of Statens vegvesen (daily traffic and share of long vehicles per section). A quarter of the long vehicles are assigned to the medium class, the rest to heavy. Oslo also has counts from the [EU city traffic dataset](https://github.com/XavB64/traffic-volume-data-EU-cities).

Municipal streets have no counts. Uncounted streets use the class default, adjusted for surrounding buildings and the counted roads they connect to.

## Railways

Train and tram counts: Norwegian national timetable from [public-transport.earth](https://data.public-transport.earth/gtfs/no), one busy Wednesday. Freight is not covered: the timetable is passenger-only.

## Industry

Wind turbines are OpenStreetMap points, matched within 500 m to turbines of operating wind farms in the [NVE register](https://www.nve.no/). NVE publishes power per wind farm only; each turbine is assigned the farm's power divided by its number of turbines.

## Buildings and terrain

Kartverket has measured building heights, available only as county files behind an order form; they are not loaded. Heights: OpenStreetMap tags and the typical height for the footprint size.
