---
title: Sweden
intro: National road counts (Trafikverket), city traffic counts in Stockholm and Malmö, two national passenger timetables, turbine ratings from Vindbrukskollen. Other roads are estimated from the buildings around them. Rail freight from the national train-kilometres.
map: { center: [15.5, 62.0], zoom: 5 }
---

## Roads

State and municipal roads: [NVDB Trafik](https://www.trafikverket.se/e-tjanster/hamta-data-fran-trafikverket/), the Trafikverket annual-average daily traffic per road link (CC0), with light, medium-heavy and heavy vehicles from sample measurements.

A road takes the nearest measured link part within 50 m running along it; divided-road carriageways carry one direction's flow each. Assessed rather than measured flows are skipped, and 1 % of each total is assigned to motorcycles.

Stockholm and Malmö: [EU city traffic dataset](https://github.com/XavB64/traffic-volume-data-EU-cities).

Uncounted streets carry the trips their buildings make. Uncounted main roads follow one model of the trips made around them, fitted on counted roads.

Vehicle mix: where not counted, the medium and heavy vehicles that counted roads of the same class carry; motorcycles 1.1 % of a town street's traffic (WHO 2023); electric cars 7.5 % of cars, rolling noise only (IEA 2024).

Unsigned roads: a primary road 50 km/h in towns, 70 outside, a motorway 110.

## Railways

Train, tram and metro counts: the Swedish feed on [public-transport.earth](https://data.public-transport.earth/gtfs/se) and [GTFS Sverige 2](https://api.resrobot.se/gtfs/sweden.zip) from Trafiklab, one busy Wednesday. The timetables are passenger-only. Where no count exists, the class defaults are scaled to the country's train-kilometres (Eurostat 2024); freight comes from them, drawn to the TEN-T corridors, 37 % of it at night as at Germany's railway monitors.

## Industry

Wind turbines are OpenStreetMap points. A turbine within 200 m of a built turbine in the [Vindbrukskollen register](https://vbk.lansstyrelsen.se/) takes its rated power and hub height from the register; the others have an unknown rating.

The register adds 393 turbines OpenStreetMap lacks and removes 57 it calls dismantled.

## Buildings and terrain

Terrain from Lantmäteriet's laser survey.

## Homes and places

Homes mapped in OpenStreetMap: 38 heat pumps and 4 air conditioners per 100 homes (the region's median). Church bells ring three times a day, with a Sunday peal. Mosques: only those with a mapped minaret call, once on Fridays.
