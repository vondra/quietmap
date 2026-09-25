---
title: Sweden
intro: National road counts (Trafikverket), city traffic counts in Stockholm and Malmö, two national passenger timetables, turbine ratings from Vindbrukskollen. Other roads use class defaults. Rail freight not covered.
map: { center: [15.5, 62.0], zoom: 5 }
---

## Roads

State and municipal roads: [NVDB Trafik](https://www.trafikverket.se/e-tjanster/hamta-data-fran-trafikverket/), the Trafikverket annual-average daily traffic per road link (CC0), with light, medium-heavy and heavy vehicles from sample measurements.

A road takes the nearest measured link part within 50 m running along it; divided-road carriageways carry one direction's flow each. Assessed rather than measured flows are skipped, and 1 % of each total is assigned to motorcycles.

Stockholm and Malmö: [EU city traffic dataset](https://github.com/XavB64/traffic-volume-data-EU-cities).

Uncounted streets use the class default, adjusted for surrounding buildings and the counted roads they connect to.

## Railways

Train, tram and metro counts: the Swedish feed on [public-transport.earth](https://data.public-transport.earth/gtfs/se) and [GTFS Sverige 2](https://api.resrobot.se/gtfs/sweden.zip) from Trafiklab, one busy Wednesday. Freight is not covered: the timetables are passenger-only.

## Industry

Wind turbines are OpenStreetMap points. A turbine within 200 m of a built turbine in the [Vindbrukskollen register](https://vbk.lansstyrelsen.se/) takes its rated power and hub height from the register; the others have an unknown rating.
