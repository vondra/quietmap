---
title: Brazil
intro: No traffic counts; main roads use the world defaults, other roads are estimated from the buildings around them. No train timetable is loaded. Power plants from the ANEEL SIGA register.
---

## Roads

No traffic counts are loaded: the DNIT national count programme is served from hosts unreachable outside Brazil. Motorways, trunks and primaries use the [world defaults](/about/data-preparation), which are fitted on counted roads in other countries. Where buildings are mapped, secondary and tertiary roads follow the trips made within 5 km of them, fitted on European counts, and residential, service and unclassified streets carry the trips of the buildings each street serves.

Vehicle mix: where not counted, a main road takes the medium and heavy shares of counted roads of its class, a street 1 % medium and 2 % heavy; motorcycles 7.7 % of a town street's traffic (WHO 2023); electric cars 0.3 % of cars, rolling noise only (IEA 2024); lorries and buses at most 90 km/h.

Unsigned roads: a primary road 60 km/h in towns, 80 outside.

## Railways

No timetable is loaded. Railways, including surface metro sections, use the
[world defaults](/about/data-preparation). These are not tuned to Brazilian ore railways
such as Carajás and Vitória a Minas.

## Industry

Power plants: the ANEEL SIGA register, published as open map layers of thermal, hydro, nuclear and solar plants; plants in operation only. Each OSM industrial area takes the nearest plant within 2 km, in the order thermal, hydro, nuclear, solar.

Mines, refineries and steelworks are OSM industrial areas with a type inferred from name and tags, plus the steel plants, cement plants and coal mines listed by Global Energy Monitor. No open mining register is loaded for Brazil.

Wind turbines are OSM points. The ANEEL per-turbine data (hub height, rated power) is not merged; a turbine without tagged specs takes the fleet medians, 105 dB(A) at full power and a 105 m hub.

## Ships

Coast and Amazon: Global Fishing Watch AIS vessel density.

## Homes and places

Homes mapped in OpenStreetMap: 16 air conditioners per 100 homes. Mosques: only those with a mapped minaret call, once on Fridays.
