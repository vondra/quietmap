---
title: Denmark
intro: National and municipal road counts (Mastra), one passenger timetable for all rail, turbine ratings from the Energistyrelsen register. Rail freight from the national train-kilometres.
map: { center: [9.5, 56.0], zoom: 7 }
---

## Roads

Traffic volumes: [Mastra](https://www.opendata.dk/vejdirektoratet/taellinger-nogletal-mastra), the counting system of Vejdirektoratet and the municipalities (yearly average traffic and number of trucks per station). Mastra does not split trucks further; 5 % are assigned to the medium class, the rest to heavy.

Copenhagen also has counts from the [EU city traffic dataset](https://github.com/XavB64/traffic-volume-data-EU-cities). Uncounted streets carry the trips their buildings make. Uncounted main roads follow one model of the trips made around them, fitted on counted roads.

Vehicle mix: where not counted, the medium and heavy vehicles that counted roads of the same class carry; motorcycles 1.1 % of a town street's traffic (the region's median); electric cars 12.6 % of cars, rolling noise only (IEA 2024).

Unsigned roads: a primary road 50 km/h in towns, 80 outside (traffic drives 68 on a two-way one), a motorway 130.

## Railways

Train, S-tog, metro and light rail counts: [Rejseplanen timetable](https://www.rejseplanen.info/labs/GTFS.zip), one busy Wednesday. The timetable is passenger-only. Where no count exists, the class defaults are scaled to the country's train-kilometres (Eurostat 2024); freight comes from them, drawn to the TEN-T corridors, 37 % of it at night as at Germany's railway monitors. Most of the Copenhagen metro runs in tunnels, which emit no noise.

## Industry

Wind turbines are OpenStreetMap points. A turbine within 200 m of an [Energistyrelsen register](https://ens.dk/) entry takes its rated power and hub height from the register.

The register adds 76 turbines OpenStreetMap lacks and removes 92 it calls dismantled.

## Buildings and terrain

The building register BBR has floors and heights for every building; the bulk download requires an account and is not loaded. Heights: OpenStreetMap tags and the typical height for the footprint size.

Terrain from Denmark's laser survey (DHM).

## Homes and places

Homes mapped in OpenStreetMap: 19 heat pumps and 4 air conditioners per 100 homes (the region's median). Church bells ring three times a day, with a Sunday peal. Mosques: only those with a mapped minaret call, once on Fridays.
