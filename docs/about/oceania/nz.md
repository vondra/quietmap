---
title: New Zealand
intro: NZTA state highway traffic and Auckland Transport street counts, both 2024. No train timetable is loaded; railways use class defaults.
map: { center: [172.0, -41.0], zoom: 5 }
---

## Roads

Traffic volumes, both 2024 editions:

- State highways: [NZTA Waka Kotahi open data portal](https://opendata-nzta.opendata.arcgis.com/). Each carriageway section has an estimated daily traffic and a heavy vehicle percentage.
- Auckland streets: [Auckland Transport](https://data-atgis.opendata.arcgis.com/) count sites, each with a daily count and a heavy vehicle percentage.

An OSM motorway, trunk, primary, secondary or tertiary road takes the nearest count within 200 m with a compatible road class. The heavy share is the published percentage; one fifth of it is assigned to medium trucks, an assumed split.

Local streets outside Auckland have no counts loaded and use class defaults. Motorways, trunks and primaries without a count use the world estimate per lane ([world defaults](/about/methodology)).

## Railways

No timetable is loaded. The GTFS feeds of Auckland Transport and Metlink Wellington are not yet integrated. All lines use the [world railway defaults](/about/methodology). Lines without a usage tag use the [unclassified railway default](/about/methodology).

## Industry

Power plants: WRI Global Power Plant Database. A plant is a noise source only where OSM maps an industrial area at its location. Other sites are OSM industrial areas with a type inferred from name and tags. No usable national pollutant register exists.

No open per-turbine register was found. Wind turbines are OSM points; a turbine without tagged specs is treated as a 2 MW machine.

## Ships

Coast and Cook Strait: Global Fishing Watch AIS vessel density.
