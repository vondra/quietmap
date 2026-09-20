---
title: United States
intro: FHWA HPMS 2022 traffic counts on federal-aid highways, Amtrak timetable, USGS wind turbine database. Local streets, commuter rail and freight use class defaults.
map: { center: [-98.0, 39.0], zoom: 4 }
---

## Roads

Traffic volumes: [FHWA Highway Performance Monitoring System](https://www.fhwa.dot.gov/policyinformation/hpms.cfm), 2022 edition, read from the public [HPMS feature service](https://services.arcgis.com/xOi1kZaI0eWDREZv/ArcGIS/rest/services/HPMS_FULL_US_2022_Sysnomulti_view/FeatureServer/0). An OSM road takes the nearest HPMS segment within 200 m with a compatible functional class.

HPMS publishes one total per segment. The heavy-vehicle share is assigned by functional class:

| HPMS functional class | Heavy share |
|---|---:|
| 1 Interstate | 12% |
| 2 Other freeway or expressway | 10% |
| 3 Other principal arterial | 8% |
| 4 Minor arterial | 6% |
| 5 Major collector | 5% |

Motorcycles are 1% of vehicles.

Day, evening and night split: [FHWA TMAS](https://www.fhwa.dot.gov/policyinformation/tables/tmasdata/) hourly station counts for 2025. A road within 200 m of a station on the same route uses that station's split. TMAS publishes hourly totals only; trucks are assumed to follow the same hourly profile as cars.

Minor collectors and local streets are not in HPMS and use class defaults. Motorways, trunks and primaries without a count use the world estimate per lane ([world defaults](/about/methodology)).

## Railways

Passenger trains: [Amtrak GTFS timetable](https://content.amtrak.com/content/gtfs/GTFS.zip). Commuter rail feeds (LIRR, NJ Transit, Metra, MBTA, Caltrain and others) are not loaded; those lines use the [world railway defaults](/about/methodology). Lines without a usage tag use the [unclassified railway default](/about/methodology). Surface metro sections are included; see the [railway method](/about/methodology).

Freight is not covered: BNSF, Union Pacific, CSX and Norfolk Southern publish no schedules.

## Industry

Wind turbines: [US Wind Turbine Database](https://eerscmap.usgs.gov/uswtdb/) from USGS, LBNL and DOE. An OSM turbine without specs takes rated power and hub height from the nearest database turbine within 500 m.

Power plants: WRI Global Power Plant Database. Steel plants, cement plants and coal mines: Global Energy Monitor. Other factories are OSM industrial areas with an inferred type. The EPA Toxics Release Inventory is not used.

## Ships

Global Fishing Watch AIS vessel density.
