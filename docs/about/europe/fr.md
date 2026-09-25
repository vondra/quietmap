---
title: France
intro: National road census (Cerema TMJA), street counts in eleven cities, SNCF national and Transilien timetables. Paris metro, city trams and rail freight not covered.
map: { center: [2.5, 46.6], zoom: 6 }
---

## Roads

- Autoroutes and routes nationales: [Cerema TMJA census](https://www.data.gouv.fr/), 2024 for the concession motorways and 2019 for the rest of the national network (daily traffic and truck share per section). Where a section has no truck share (680 sections of 2019, including the A4 and A86 near Paris), the nearest section of the same road lends its share.
- Paris, Lyon, Lille, Bordeaux, Marseille, Toulouse, Grenoble, Rennes, Rouen, Montpellier: [EU city traffic dataset](https://github.com/XavB64/traffic-volume-data-EU-cities).

Routes départementales and other city streets have no counts. Uncounted streets use the class default, adjusted for surrounding buildings and the counted roads they connect to.

## Railways

Train counts: two [SNCF timetables](https://data.sncf.com/), one busy Wednesday. The national feed covers TGV, Intercités and TER; the Transilien feed adds the RER and Transilien trains of the Paris region.

The Paris metro and city trams are not covered: no RATP feed is loaded. Their surface sections use the class default. Freight is not covered: the timetables are passenger-only.
