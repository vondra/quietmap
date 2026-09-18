---
title: Mexico
intro: Datos Viales 2025 traffic counts with a measured vehicle mix on federal and state highways, Mexico City rail from the SEMOVI timetable. Other roads, railways and wind turbines use defaults.
map: { center: [-99.0, 23.5], zoom: 5 }
---

## Roads

Traffic volumes: Datos Viales 2025, the annual counts of SICT and the Instituto Mexicano del Transporte. Each counted section has TDPA (annual average daily traffic) and a measured vehicle mix: cars, buses, trucks by axle count, motorcycles. A section without a published mix takes the national average: 79.5% light, 7.1% medium, 8.2% heavy, 5.2% motorcycles. SICT offers no download; the data is read from a community copy under CC BY 4.0.

An OSM motorway, trunk, primary or secondary road takes the nearest counted section within 200 m with a compatible road type: a federal toll road matches a motorway or trunk, a free federal road a trunk or primary, a state road a primary or secondary. Roads outside Mexico never match.

Local roads are not in the dataset and use world defaults. Motorways, trunks and primaries without a count use the world default scaled by 1.286.

## Railways

Mexico City: [SEMOVI unified GTFS](https://datos.cdmx.gob.mx/dataset/gtfs), covering Tren Ligero and Tren Suburbano. Headway-based entries are expanded into daily train counts. Lines tagged railway=subway, including the Metro, are not included.

No timetable is loaded for the Guadalajara and Monterrey light rail or for Tren Maya, and freight railways publish no schedules. These lines use class defaults.

## Industry

Power plants: Global Energy Monitor power tracker, operating plants only. Refineries and factories are OSM industrial areas with an inferred type. The federal pollutant register RETC has no bulk download and is not used.

No open per-turbine register exists for Mexico. Wind turbines are OSM points; a turbine without tagged specs is treated as a 2 MW machine.

## Ships

Global Fishing Watch AIS vessel density.
