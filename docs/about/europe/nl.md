---
title: Netherlands
intro: National motorway counts (INWEVA), modelled city traffic in Amsterdam, national passenger timetable. Other roads use class defaults. Rail freight not covered; 3DBAG building heights not used yet.
map: { center: [5.3, 52.2], zoom: 7 }
---

## Roads

Motorways and national roads: [INWEVA](https://www.nationaalgeoregister.nl/geonetwork/srv/api/records/93e99016-9b53-45d6-8b3c-fc9bf8086256), the 2024 Rijkswaterstaat section intensities (CC0), with cars, vans and two truck classes per direction from the loop detectors.

A motorway takes the counted section of its number within 50 m running along it; only sections measured at their own loops count, derived neighbours keep the prior. The detectors cannot see motorcycles, so 1 % of each total is assigned to them.

Amsterdam: the 2025 Amsterdam file of the [EU city traffic dataset](https://github.com/XavB64/traffic-volume-data-EU-cities). These values come from the city's traffic model, not from counters, and are shown as modelled.

Uncounted streets use the class default, adjusted for surrounding buildings and the counted roads they connect to.

## Railways

Train, tram and metro counts: Dutch national timetable from [public-transport.earth](https://data.public-transport.earth/gtfs/nl), one busy Wednesday. Freight is not covered: the timetable is passenger-only.

## Buildings and terrain

[3DBAG](https://3dbag.nl/) has measured heights for every building but is not used yet. Heights: OpenStreetMap tags and the typical height for the footprint size.
