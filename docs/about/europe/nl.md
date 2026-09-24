---
title: Netherlands
intro: Modelled city traffic in Amsterdam, national passenger timetable. Other roads use class defaults. Rail freight not covered; 3DBAG building heights not used yet.
map: { center: [5.3, 52.2], zoom: 7 }
---

## Roads

Amsterdam: the 2025 Amsterdam file of the [EU city traffic dataset](https://github.com/XavB64/traffic-volume-data-EU-cities). These values come from the city's traffic model, not from counters, and are shown as modelled.

Elsewhere motorways, trunk and primary roads use the world estimate per lane; smaller roads use class defaults ([world defaults](/about/methodology)). The national NDW counts require registration and are not loaded.

## Railways

Train, tram and metro counts: Dutch national timetable from [public-transport.earth](https://data.public-transport.earth/gtfs/nl), one busy Wednesday. Freight is not covered: the timetable is passenger-only.

## Buildings and terrain

[3DBAG](https://3dbag.nl/) has measured heights for every building but is not used yet. Heights: OpenStreetMap tags and the GHSL average for the block.
