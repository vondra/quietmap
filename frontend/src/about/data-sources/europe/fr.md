---
title: France
intro: National road census (Cerema TMJA), street counts in ten cities, SNCF national and Transilien timetables. Paris metro and city trams not covered; rail freight from the national train-kilometres.
---

## Roads

- Autoroutes and routes nationales: [Cerema TMJA census](https://www.data.gouv.fr/), 2024 for the concession motorways and 2019 for the rest of the national network (daily traffic and truck share per section). Where a section has no truck share (680 sections of 2019, including the A4 and A86 near Paris), the nearest section of the same road lends its share.
- Paris, Lyon, Lille, Bordeaux, Marseille, Toulouse, Grenoble, Rennes, Rouen, Montpellier: [EU city traffic dataset](https://github.com/XavB64/traffic-volume-data-EU-cities).

Routes départementales and other city streets have no counts. Uncounted streets carry the trips their buildings make. Uncounted motorways, trunk and primary roads follow one model of the trips made around them, fitted on counted roads, and secondary and tertiary roads a fit of the trips made within 5 km.

Vehicle mix: where not counted, a main road takes the medium and heavy shares of counted roads of its class, a street 1 % medium and 2 % heavy; motorcycles 1.1 % of a town street's traffic (WHO 2023); electric cars 3.1 % of cars, rolling noise only (IEA 2024); lorries and buses at most 90 km/h.

Unsigned roads: a primary road 50 km/h in towns, 80 outside (cars drive 68 on a two-way one), a motorway 130.

## Railways

Train counts: two [SNCF timetables](https://data.sncf.com/), one busy Wednesday. The national feed covers TGV, Intercités and TER; the Transilien feed adds the RER and Transilien trains of the Paris region.

The Paris metro and city trams are not covered: no RATP feed is loaded. Their surface sections use the class default. The timetables are passenger-only. Where no count exists, the class defaults are scaled to the country's train-kilometres (Eurostat 2024); freight comes from them, drawn to the TEN-T corridors, 37 % of it at night as at Germany's railway monitors.

## Buildings and terrain

Terrain from IGN's RGE ALTI.

## Homes and places

Homes mapped in OpenStreetMap: 20 heat pumps and 24 air conditioners per 100 homes. Church bells ring three times a day, with a Sunday peal. Mosques: only those with a mapped minaret call, once on Fridays. Bars without mapped hours close at 02:00.

## Checked against

Bruitparif's stations; Paris's street stations.
