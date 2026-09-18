---
title: Argentina
intro: DNV traffic census 2017-18 on main routes; elsewhere estimates from road surface and type in the IGN network. No train timetable is loaded.
map: { center: [-64, -38], zoom: 4 }
---

## Roads

Traffic volumes: the Dirección Nacional de Vialidad 2017-18 census (TMDA), published as a map layer at [IDE Transporte](https://ide.transporte.gob.ar/geoserver/observ/ows). Newer years exist only as PDF reports. An OSM motorway, trunk or primary road takes the nearest counted section within 300 m. Sections with fewer than 50 vehicles per day are ignored.

Roads without a nearby count are matched within 400 m to the national and provincial route network of the [Instituto Geográfico Nacional](https://wms.ign.gob.ar/geoserver/transporte/ows), which gives surface and road type but no traffic.

| Road in the IGN network | Vehicles per day |
|---|---:|
| Paved autopista or autovía | 30,000 |
| Paved national route | 18,000 |
| Paved provincial route | 12,000 |
| Gravel or dirt | 3,000 |

Both counts and estimates are doubled inside Greater Buenos Aires and Córdoba, and multiplied by 1.4 in 22 other cities: Rosario, Mendoza, San Miguel de Tucumán, La Plata, Mar del Plata, Salta, Santa Fe, San Juan, Resistencia, Neuquén, Bahía Blanca, Posadas, Corrientes, Paraná, Santiago del Estero, San Salvador de Jujuy, Río Cuarto, Comodoro Rivadavia, San Luis, La Rioja, Catamarca and Formosa. The city boxes are drawn manually.

The census publishes totals only. The vehicle mix is an estimate:

| Where | Light | Medium | Heavy | Motorcycle |
|---|---:|---:|---:|---:|
| Buenos Aires, Córdoba | 75% | 10% | 10% | 5% |
| The 22 cities | 73% | 10% | 12% | 5% |
| Everywhere else | 60% | 10% | 27% | 3% |

Secondary and smaller roads use world defaults.

## Railways

No timetable is loaded. The only open feed is a 2019 archive of the [Buenos Aires Subte GTFS](https://buenosaires.gob.ar/sites/gcaba/files/subte_gtfs.zip), and Subte lines are tagged railway=subway, which is not included.

The Trenes Argentinos commuter lines out of Retiro, Once and Constitución and the freight operators publish no loadable feed. All lines use class defaults: 80 passenger and 20 freight trains per day on a main line, 30 and 5 on a branch.

## Industry

Power plants: Global Energy Monitor, operating plants only, where OSM maps an industrial area at the site. Other industry: OSM industrial areas with a type inferred from name and tags.

No open per-turbine register exists for Argentina. Wind turbines are OSM points; a turbine without tagged specs is treated as a 2 MW machine.

## Ships

Coast and Río de la Plata: Global Fishing Watch AIS vessel density.
