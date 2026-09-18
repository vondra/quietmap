---
title: Uruguay
intro: Roads use OSM class defaults scaled by 0.722, a factor derived from population density. No train timetable is loaded. Power plants from Global Energy Monitor.
map: { center: [-56, -33], zoom: 6 }
---

## Roads

No open traffic counts; the transport ministry's map server is unreachable from abroad. Roads use OSM class defaults. Motorways, trunks, primaries and their ramps are scaled by 0.722. No vehicles-per-kilometre figure is available for Uruguay, so the factor is derived from population density, a weaker proxy.

| OSM class | Default vehicles per day |
|---|---:|
| Motorway | 30,000 × 0.722 = 21,660 |
| Trunk | 15,000 × 0.722 = 10,830 |
| Primary | 9,000 × 0.722 = 6,498 |
| Secondary | 3,000 |
| Tertiary | 800 |
| Residential | 500 |
| Unclassified | 1,340 |
| Service | 250 |
| Track | 5 |

Where buildings are mapped, residential, living, service and unclassified streets are instead estimated from the buildings they serve.

## Railways

No timetable is loaded. Every line that OSM maps as a working railway takes the class default: 80 passenger and 20 freight trains per day on a main line, 30 and 5 on a branch. This applies equally to the freight-only Ferrocarril Central, opened in 2023 for the pulp mill at Paso de los Toros, and to little-used inland lines.

## Industry

Power plants: Global Energy Monitor, operating plants only, where OSM maps an industrial area at the site. The three pulp mills at Fray Bentos, Conchillas and Paso de los Toros are therefore classed by their power plants. The La Teja refinery and other sites are OSM industrial areas with a type inferred from name and tags.

Wind turbines are OSM points; a turbine without tagged specs is treated as a 2 MW machine.

## Ships

Río de la Plata and Atlantic coast: Global Fishing Watch AIS vessel density.
