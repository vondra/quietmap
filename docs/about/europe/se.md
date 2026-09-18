---
title: Sweden
intro: City traffic counts in Stockholm and Malmö, two national passenger timetables, turbine ratings from Vindbrukskollen. Other roads use class defaults. Rail freight not covered.
map: { center: [15.5, 62.0], zoom: 5 }
---

## Roads

Stockholm and Malmö: [EU city traffic dataset](https://github.com/XavB64/traffic-volume-data-EU-cities).

Elsewhere class defaults apply, with motorway, trunk and primary scaled by 0.829 (vehicles per kilometre of road); the motorway default is 24,870 vehicles per day. Trafikverket has counts for every state road; the download requires registration and is not loaded.

## Railways

Train, tram and metro counts: the Swedish feed on [public-transport.earth](https://data.public-transport.earth/gtfs/se) and [GTFS Sverige 2](https://api.resrobot.se/gtfs/sweden.zip) from Trafiklab, one busy Wednesday. Freight is not covered: the timetables are passenger-only.

## Industry

Wind turbines are OpenStreetMap points. A turbine within 200 m of a built turbine in the [Vindbrukskollen register](https://vbk.lansstyrelsen.se/) takes its rated power and hub height from the register; the others have an unknown rating.
