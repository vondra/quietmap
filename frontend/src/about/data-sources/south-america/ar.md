---
title: Argentina
intro: DNV traffic census 2017-18 on main routes; elsewhere estimates from road surface and type in the IGN network. The only train timetable is the Buenos Aires Subte's 2019 archive.
---

## Roads

Traffic volumes: the Dirección Nacional de Vialidad 2017-18 census (TMDA), published as a map layer at [IDE Transporte](https://ide.transporte.gob.ar/geoserver/observ/ows). Newer years exist only as PDF reports. An OSM motorway, trunk or primary road takes the nearest counted section within 300 m. Sections with fewer than 50 vehicles per day are ignored. Counts reach 62 % of the motorway, trunk and primary length.

Roads without a nearby count are matched within 400 m to the national and provincial route network of the [Instituto Geográfico Nacional](https://wms.ign.gob.ar/geoserver/transporte/ows), which gives surface and road type but no traffic.

| Road in the IGN network | Vehicles per day |
|---|---:|
| Paved autopista or autovía | 30,000 |
| Paved national route | 18,000 |
| Paved provincial route | 12,000 |
| Gravel or dirt | 3,000 |

Counts are used as published. Estimates are doubled inside Greater Buenos Aires and Córdoba, and multiplied by 1.4 in 22 other cities: Rosario, Mendoza, San Miguel de Tucumán, La Plata, Mar del Plata, Salta, Santa Fe, San Juan, Resistencia, Neuquén, Bahía Blanca, Posadas, Corrientes, Paraná, Santiago del Estero, San Salvador de Jujuy, Río Cuarto, Comodoro Rivadavia, San Luis, La Rioja, Catamarca and Formosa. The city boxes are drawn manually.

The census publishes totals only; counted sections take an estimated vehicle mix:

| Where | Light | Medium | Heavy | Motorcycle |
|---|---:|---:|---:|---:|
| Buenos Aires, Córdoba | 75% | 10% | 10% | 5% |
| The 22 cities | 73% | 10% | 12% | 5% |
| Everywhere else | 60% | 10% | 27% | 3% |

Vehicle mix: where not counted, a main road takes the medium and heavy shares of counted roads of its class, a street 1 % medium and 2 % heavy; motorcycles 16 % of a town street's traffic (WHO 2023).

Motorways, trunks and primaries matched to neither use the world estimate per lane; secondary and tertiary roads follow the trips made within 5 km of them, streets the trips their buildings make, else world defaults.

Unsigned roads: a primary road 60 km/h in towns, 110 outside (cars drive 93 on a two-way one), an expressway 120 (101 on a two-way one), a motorway 130.

## Railways

The 2019 archive of the [Buenos Aires Subte GTFS](https://buenosaires.gob.ar/sites/gcaba/files/subte_gtfs.zip) is loaded; of its lines only the Premetro runs on the surface, the underground ones emit nothing. Surface metro sections are included; see the [railway method](/about/data-preparation).

The Trenes Argentinos commuter lines out of Retiro, Once and Constitución and the freight operators publish no loadable feed. All other lines use the [world railway defaults](/about/data-preparation).

## Industry

Power plants: Global Energy Monitor, operating plants only, where OSM maps an industrial area at the site. Other industry: OSM industrial areas with a type inferred from name and tags.

No open per-turbine register exists for Argentina. Wind turbines are OSM points; a turbine without tagged specs takes the fleet medians, 105 dB(A) at full power and a 105 m hub.

## Ships

Coast and Río de la Plata: Global Fishing Watch AIS vessel density.

## Homes and places

Homes mapped in OpenStreetMap: 16 air conditioners per 100 homes (the region's median). Mosques: only those with a mapped minaret call, once on Fridays.
