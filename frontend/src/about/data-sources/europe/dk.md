---
title: Denmark
intro: National and municipal road counts (Mastra), one passenger timetable for all rail, turbine ratings from the Energistyrelsen register. Rail freight from the national train-kilometres.
---

## Roads

Traffic volumes: [Mastra](https://www.opendata.dk/vejdirektoratet/taellinger-nogletal-mastra), the counting system of Vejdirektoratet and the municipalities (yearly average traffic and number of trucks per station). Mastra does not split trucks further; 5 % are assigned to the medium class, the rest to heavy.

Copenhagen also has counts from the [EU city traffic dataset](https://github.com/XavB64/traffic-volume-data-EU-cities). Uncounted streets carry the trips their buildings make. Uncounted motorways, trunk and primary roads follow one model of the trips made around them, fitted on counted roads, and secondary and tertiary roads a fit of the trips made within 5 km.

Vehicle mix: where not counted, a main road takes the medium and heavy shares of counted roads of its class, a street 1 % medium and 2 % heavy; motorcycles 1.1 % of a town street's traffic (the region's median); electric cars 12.6 % of cars, rolling noise only (IEA 2024).

Unsigned roads: a primary road 50 km/h in towns, 80 outside (cars drive 68 on a two-way one), a motorway 130.

## Railways

Train, S-tog, metro and light rail counts: [Rejseplanen timetable](https://www.rejseplanen.info/labs/GTFS.zip), one busy Wednesday. The timetable is passenger-only. Lines without a count take the class defaults; freight comes from them, scaled to the country's goods train-kilometres (Eurostat 2024), drawn to the TEN-T corridors, 37 % of it at night as at Germany's railway monitors. Most of the Copenhagen metro runs in tunnels, which emit no noise.

## Industry

Wind turbines are OpenStreetMap points. A turbine paired with an [Energistyrelsen register](https://ens.dk/) entry, one to one and nearest first within 500 m, takes its rated power and hub height from the register.

The register adds 76 turbines OpenStreetMap lacks and removes 92 it calls dismantled.

## Buildings and terrain

The building register BBR has floors and heights for every building; the bulk download requires an account and is not loaded. Heights: OpenStreetMap tags and the typical height for the footprint size.

Terrain from Denmark's laser survey (DHM).

## Homes and places

Homes mapped in OpenStreetMap: 19 heat pumps and 4 air conditioners (the region's median) per 100 homes. Church bells ring three times a day, with a Sunday peal. Mosques: only those with a mapped minaret call, once on Fridays.
