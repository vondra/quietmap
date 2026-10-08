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

Local streets outside Auckland have no counts loaded and carry the trips their buildings make. Motorways, trunks and primaries without a count follow one model of the trips made around them, fitted on counted roads ([method](/about/methodology)).

Vehicle mix: where not counted, the medium and heavy vehicles that counted roads of the same class carry; motorcycles 3.4 % of a town street's traffic (the region's median); electric cars 2.3 % of cars, rolling noise only (IEA 2024); lorries and buses at most 90 km/h.

Unsigned roads: a primary road 50 km/h in towns, 100 outside (traffic drives 84 on a two-way one).

## Railways

No timetable is loaded. The GTFS feeds of Auckland Transport and Metlink Wellington are not yet integrated. All lines use the [world railway defaults](/about/methodology). Lines without a usage tag use the [unclassified railway default](/about/methodology).

## Industry

Power plants: WRI Global Power Plant Database. A plant is a noise source only where OSM maps an industrial area at its location. Other sites are OSM industrial areas with a type inferred from name and tags. No usable national pollutant register exists.

No open per-turbine register was found. Wind turbines are OSM points; a turbine without tagged specs is treated as a 2 MW machine.

## Ships

Coast and Cook Strait: Global Fishing Watch AIS vessel density.

## Homes and places

Homes mapped in OpenStreetMap: 49 heat pumps and 59 air conditioners per 100 homes (the region's median). Mosques: only those with a mapped minaret call, once on Fridays.
