---
title: United States
intro: FHWA HPMS 2022 traffic counts on federal-aid highways, Amtrak timetable, FRA crossing horns, USGS wind turbine database. Local streets are estimated from their buildings; commuter rail and freight use class defaults.
---

## Roads

Traffic volumes: [FHWA Highway Performance Monitoring System](https://www.fhwa.dot.gov/policyinformation/hpms.cfm), 2022 edition, read from the public [HPMS feature service](https://services.arcgis.com/xOi1kZaI0eWDREZv/ArcGIS/rest/services/HPMS_FULL_US_2022_Sysnomulti_view/FeatureServer/0). An OSM road takes the nearest HPMS segment line within 100 m that runs along it and has a compatible functional class. A one-way segment counts one direction, unless it repeats the total of a two-way segment of the same road within 300 m; then it is that road's two-way total.

HPMS publishes one total per segment; the vehicle mix comes from FHWA's state shares (below).

Day, evening and night split: [FHWA TMAS](https://www.fhwa.dot.gov/policyinformation/tables/tmasdata/) hourly counts of 8,056 stations in 2025, one split per road group (motorways and trunks, primary roads, other roads) for the whole country. TMAS publishes hourly totals only; trucks are assumed to follow the same hourly profile as cars.

Minor collectors and local streets are not in HPMS, whose values reach 60 % of the length of motorways, trunks and primaries and 1 % of secondary, tertiary and unclassified roads. Uncounted local streets carry the trips their buildings make, secondary and tertiary roads the trips made within 5 km of them. Motorways, trunks and primaries without a count follow one model of the trips made around them, fitted on counted roads ([method](/about/data-preparation)).

Vehicle mix: where not counted, FHWA's state mix by road group and area (table VM-4), motorcycles included; electric cars 1.9 % of cars, rolling noise only (IEA 2024); lorries and buses at most 105 km/h.

Unsigned roads: a primary road 64 km/h in towns, 89 outside (cars drive 75 on a two-way one).

## Railways

Passenger trains: [Amtrak GTFS timetable](https://content.amtrak.com/content/gtfs/GTFS.zip). Commuter rail feeds (LIRR, NJ Transit, Metra, MBTA, Caltrain and others) are not loaded; those lines use the [world railway defaults](/about/data-preparation). Lines without a usage tag use the [unclassified railway default](/about/data-preparation). Surface metro sections are included; see the [railway method](/about/data-preparation).

Freight: BNSF, Union Pacific, CSX and Norfolk Southern publish no schedules; main lines take the class default of 20 freight trains a day, branches 5.

Level-crossing horns: [FRA Highway-Rail Crossing Inventory](https://data.transportation.gov/Railroads/Crossing-Inventory-Data-Form-71-Current/m2f8-22s6) (Form 6180.71). Public at-grade crossings with trains sound on approach; full-day quiet zones and Chicago-excused crossings stay silent, partial zones at night.

## Industry

Wind turbines: [US Wind Turbine Database](https://eerscmap.usgs.gov/uswtdb/) from USGS, LBNL and DOE. Each database turbine is paired with one OSM turbine within 500 m, nearest pairs first, and gives it the database's rated power and hub height.

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
