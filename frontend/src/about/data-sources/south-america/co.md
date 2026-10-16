---
title: Colombia
intro: INVIAS 2024 traffic census with a measured bus and truck share on national roads, estimates from road administration elsewhere. No train timetable is loaded.
---

## Roads

Traffic volumes: the INVIAS 2024 traffic census, published as an [open map service](https://services6.arcgis.com/kyerLIHvrND0OSya/). Each counted section has TPDA (average daily traffic) and the percentage of cars, buses and trucks. Motorcycles are not counted; a flat 5% is added and the measured shares are scaled to 95%.

An OSM motorway, trunk or primary road takes the nearest counted section within 500 m. Sections under 50 vehicles per day are ignored. Counts reach 66 % of the motorway, trunk and primary length.

Roads without a nearby count are matched within 400 m to the INVIAS national road network and estimated from the administering body:

| Road in the national network | Vehicles per day |
|---|---:|
| Paved ANI concession, dual carriageway | 25,000 |
| Paved ANI concession, single carriageway | 18,000 |
| Paved INVIAS road | 12,000 |
| Other paved road | 6,000 |
| Unpaved | 1,500 |

Counts are used as published. Estimates are doubled inside Bogotá and Medellín and multiplied by 1.4 in 24 other cities: Cali, Barranquilla, Cartagena, Cúcuta, Bucaramanga, Pereira, Santa Marta, Ibagué, Manizales, Pasto, Villavicencio, Neiva, Armenia, Soledad, Soacha, Valledupar, Montería, Sincelejo, Buenaventura, Tunja, Riohacha, Quibdó, Florencia and Popayán. The city boxes are drawn manually.

Vehicle mix: where not counted, a main road takes the medium and heavy shares of counted roads of its class, a street 1 % medium and 2 % heavy; motorcycles 47 % of a town street's traffic (WHO 2023); electric cars 0.5 % of cars, rolling noise only (IEA 2024).

Motorways, trunks and primaries matched to neither (21 % of their length) follow one model of the trips made around them, fitted on counted roads ([method](/about/data-preparation)); secondary and tertiary roads follow the trips made within 5 km of them, streets the trips their buildings make, else world defaults.

Unsigned roads: a primary road 40 km/h in towns, 60 outside, a motorway 120, a service road 10.

## Railways

No timetable is loaded; the Cerrejón and FENOCO coal railways publish no schedule. All lines use the [world railway defaults](/about/data-preparation).

Metro de Medellín takes the light rail default of 80 trains per day where OSM tags it as light rail. Surface metro sections are included; see the [railway method](/about/data-preparation).

## Industry

Power plants: Global Energy Monitor power tracker, operating plants only. Each is attached to an OSM industrial area within 2 km; a plant with no such area nearby is not a noise source.

Mines, oil fields and refineries are OSM industrial areas with a type inferred from name and tags. The open mining titles of the Agencia Nacional de Minería and the production blocks of the Agencia Nacional de Hidrocarburos are not loaded.

## Ships

Both coasts: Global Fishing Watch AIS vessel density.

## Homes and places

Homes mapped in OpenStreetMap: 16 air conditioners per 100 homes (the region's median). Mosques: only those with a mapped minaret call, once on Fridays.
