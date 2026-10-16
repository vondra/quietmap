---
title: Mexico
intro: Datos Viales 2025 traffic counts with a measured vehicle mix on federal and state highways, Mexico City rail from the SEMOVI timetable. Other roads are estimated from the buildings around them; other railways and wind turbines use defaults.
---

## Roads

Traffic volumes: Datos Viales 2025, the annual counts of SICT and the Instituto Mexicano del Transporte. Each counted section has TDPA (annual average daily traffic) and a measured vehicle mix: cars, buses, trucks by axle count, motorcycles. A section without a published mix takes the national average: 79.5% light, 7.1% medium, 8.2% heavy, 5.2% motorcycles. SICT offers no download; the data is read from a community copy under CC BY 4.0.

An OSM motorway, trunk, primary or secondary road takes the nearest counted section within 200 m with a compatible road type: a federal toll road matches a motorway or trunk, a free federal road a trunk or primary, a state road a primary or secondary. Roads outside Mexico never match.

Counted sections reach 78 % of the motorway, trunk and primary length. Tertiary roads and streets are not in the dataset: uncounted secondary and tertiary roads follow the trips made within 5 km of them, streets the trips their buildings make. Motorways, trunks and primaries without a count follow one model of the trips made around them, fitted on the counted roads of 19 other countries; on Mexico's counted roads it reads 1.3 dB low ([method](/about/data-preparation)).

Vehicle mix: where not counted, a main road takes the medium and heavy shares of counted roads of its class, a street 1 % medium and 2 % heavy; motorcycles 2.1 % of a town street's traffic (WHO 2023); electric cars 0.2 % of cars, rolling noise only (IEA 2024).

Unsigned roads: a primary road 50 km/h in towns, 80 outside (cars drive 68 on a two-way one).

## Railways

Mexico City: [SEMOVI unified GTFS](https://datos.cdmx.gob.mx/dataset/gtfs) and Movimex's Toluca timetable, covering the Metro, Tren Ligero and part of El Insurgente. Headway-based entries are expanded into daily train counts. Surface metro sections are included; see the [railway method](/about/data-preparation).

Tren Suburbano, the Guadalajara and Monterrey light rail and Tren Maya take no timetable, and freight railways publish no schedules. These lines use class defaults.

## Industry

Power plants: Global Energy Monitor power tracker, operating plants only. Refineries and factories are OSM industrial areas with an inferred type. The federal pollutant register RETC has no bulk download and is not used.

No open per-turbine register exists for Mexico. Wind turbines are OSM points; a turbine without tagged specs takes the fleet medians, 105 dB(A) at full power and a 105 m hub.

## Ships

Global Fishing Watch AIS vessel density.

## Homes and places

Homes mapped in OpenStreetMap: 16 air conditioners per 100 homes. Mosques: only those with a mapped minaret call, once on Fridays.
