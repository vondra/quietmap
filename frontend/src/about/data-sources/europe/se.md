---
title: Sweden
intro: National road counts (Trafikverket), city traffic counts in Stockholm and Malmö, two national passenger timetables, turbine ratings from Vindbrukskollen. Other roads are estimated from the buildings around them. Rail freight from the national train-kilometres.
---

## Roads

State and municipal roads: [NVDB Trafik](https://www.trafikverket.se/e-tjanster/hamta-data-fran-trafikverket/), the Trafikverket annual-average daily traffic per road link (CC0), with light, medium-heavy and heavy vehicles from sample measurements.

A road takes the nearest measured link part within 50 m running along it; divided-road carriageways carry one direction's flow each. Assessed rather than measured flows are skipped, and 1 % of each total is assigned to motorcycles.

Stockholm and Malmö: [EU city traffic dataset](https://github.com/XavB64/traffic-volume-data-EU-cities).

Uncounted streets carry the trips their buildings make. Uncounted motorways, trunk and primary roads follow one model of the trips made around them, fitted on counted roads, and secondary and tertiary roads a fit of the trips made within 5 km.

Vehicle mix: where not counted, a main road takes the medium and heavy shares of counted roads of its class, a street 1 % medium and 2 % heavy; motorcycles 1.1 % of a town street's traffic (WHO 2023); electric cars 7.5 % of cars, rolling noise only (IEA 2024).

Unsigned roads: a primary road 50 km/h in towns, 70 outside, a motorway 110.

## Railways

Train, tram and metro counts: the Swedish feed on [public-transport.earth](https://data.public-transport.earth/gtfs/se) and [GTFS Sverige 2](https://api.resrobot.se/gtfs/sweden.zip) from Trafiklab, one busy Wednesday. The timetables are passenger-only. Lines without a count take the class defaults; freight comes from them, scaled to the country's goods train-kilometres (Eurostat 2024), drawn to the TEN-T corridors, 37 % of it at night as at Germany's railway monitors.

## Industry

Wind turbines are OpenStreetMap points. A turbine paired with a built turbine in the [Vindbrukskollen register](https://vbk.lansstyrelsen.se/), one to one and nearest first within 500 m, takes its rated power and hub height from the register; the others keep the rating OpenStreetMap tags, if any.

The register adds 393 turbines OpenStreetMap lacks and removes 57 it calls dismantled.

## Buildings and terrain

Terrain from Lantmäteriet's laser survey.

## Homes and places

Homes mapped in OpenStreetMap: 38 heat pumps and 4 air conditioners (the region's median) per 100 homes. Church bells ring three times a day, with a Sunday peal. Mosques: only those with a mapped minaret call, once on Fridays.
