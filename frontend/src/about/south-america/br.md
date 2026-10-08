---
title: Brazil
intro: No traffic counts; roads use the world defaults. No train timetable is loaded. Power plants from the ANEEL SIGA register.
map: { center: [-53, -14], zoom: 4 }
---

## Roads

No traffic counts are loaded: the DNIT national count programme is served from hosts unreachable outside Brazil. Every road uses the [world defaults](/about/methodology), which are fitted on counted roads in other countries. Residential and service streets are then estimated from the buildings each street serves.

Vehicle mix: where not counted, the medium and heavy vehicles that counted roads of the same class carry; motorcycles 7.7 % of a town street's traffic (WHO 2023); electric cars 0.3 % of cars, rolling noise only (IEA 2024); lorries and buses at most 90 km/h.

Unsigned roads: a primary road 50 km/h in towns, 80 outside.

## Railways

No timetable is loaded. Railways, including surface metro sections, use the
[world defaults](/about/methodology). These are not tuned to Brazilian ore railways
such as Carajás and Vitória a Minas.

## Industry

Power plants: the ANEEL SIGA register, published as open map layers of thermal, hydro, nuclear and solar plants; plants in operation only. Each OSM industrial area takes the nearest plant within 2 km, in the order thermal, hydro, nuclear, solar.

Mines, refineries and steelworks are OSM industrial areas with a type inferred from name and tags, plus the steel plants, cement plants and coal mines listed by Global Energy Monitor. No open mining register is loaded for Brazil.

Wind turbines are OSM points. The ANEEL per-turbine data (hub height, rated power) is not merged; a turbine without tagged specs is treated as a 2 MW machine.

## Ships

Coast and Amazon: Global Fishing Watch AIS vessel density.

## Homes and places

Homes mapped in OpenStreetMap: 16 air conditioners per 100 homes. Mosques: only those with a mapped minaret call, once on Fridays.
