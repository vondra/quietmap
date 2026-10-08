---
title: United States
intro: FHWA HPMS 2022 traffic counts on federal-aid highways, Amtrak timetable, FRA crossing horns, USGS wind turbine database. Local streets are estimated from their buildings; commuter rail and freight use class defaults.
map: { center: [-98.0, 39.0], zoom: 4 }
---

## Roads

Traffic volumes: [FHWA Highway Performance Monitoring System](https://www.fhwa.dot.gov/policyinformation/hpms.cfm), 2022 edition, read from the public [HPMS feature service](https://services.arcgis.com/xOi1kZaI0eWDREZv/ArcGIS/rest/services/HPMS_FULL_US_2022_Sysnomulti_view/FeatureServer/0). An OSM road takes the nearest HPMS segment line within 100 m that runs along it and has a compatible functional class. A one-way segment counts one direction, unless it repeats the total of a two-way segment of the same road within 300 m; then it is that road's two-way total.

HPMS publishes one total per segment; the vehicle mix comes from FHWA's state shares (below).

Day, evening and night split: [FHWA TMAS](https://www.fhwa.dot.gov/policyinformation/tables/tmasdata/) hourly station counts for 2025. A road within 200 m of a station on the same route uses that station's split. TMAS publishes hourly totals only; trucks are assumed to follow the same hourly profile as cars.

Minor collectors and local streets are not in HPMS and carry the trips their buildings make. Motorways, trunks and primaries without a count follow one model of the trips made around them, fitted on counted roads ([method](/about/methodology)).

Vehicle mix: where not counted, FHWA's state mix by road group and area (table VM-4), motorcycles included; lorries and buses at most 105 km/h.

Unsigned roads: a primary road 50 km/h in towns and outside.

## Railways

Passenger trains: [Amtrak GTFS timetable](https://content.amtrak.com/content/gtfs/GTFS.zip). Commuter rail feeds (LIRR, NJ Transit, Metra, MBTA, Caltrain and others) are not loaded; those lines use the [world railway defaults](/about/methodology). Lines without a usage tag use the [unclassified railway default](/about/methodology). Surface metro sections are included; see the [railway method](/about/methodology).

Freight is not covered: BNSF, Union Pacific, CSX and Norfolk Southern publish no schedules.

Level-crossing horns: [FRA Highway-Rail Crossing Inventory](https://data.transportation.gov/Railroads/Crossing-Inventory-Data-Form-71-Current/m2f8-22s6) (Form 6180.71). Public at-grade crossings with trains sound on approach; full-day quiet zones and Chicago-excused crossings stay silent, partial zones at night.

## Industry

Wind turbines: [US Wind Turbine Database](https://eerscmap.usgs.gov/uswtdb/) from USGS, LBNL and DOE. An OSM turbine without specs takes rated power and hub height from the nearest database turbine within 500 m.

Power plants: WRI Global Power Plant Database. Steel plants, cement plants and coal mines: Global Energy Monitor. Other factories are OSM industrial areas with an inferred type. The EPA Toxics Release Inventory is not used.

The register adds 1,610 turbines OpenStreetMap lacks.

## Ships

Global Fishing Watch AIS vessel density.

## Buildings and terrain

Terrain from USGS's 3DEP (the 48 states and Alaska); noise barriers from OpenStreetMap and the inventories of Washington, Florida and Virginia.

## Homes and places

Homes: heat pumps and air conditioners as the RECS survey counts them. Mosques: only those with a mapped minaret call, once on Fridays. Bars without mapped hours close at 02:00.

## Checked against

Minneapolis–St Paul airport's monitors.
