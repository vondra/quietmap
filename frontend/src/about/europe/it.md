---
title: Italy
intro: Anas census on motorways and state roads, passenger timetables for five regions. Railways in Lazio, Campania, Veneto and Sicily use class defaults. Rail freight from the national train-kilometres.
map: { center: [12.5, 42.5], zoom: 6 }
---

## Roads

Motorways and state roads: [Anas](https://www.stradeanas.it/) daily traffic census (TGM), one total per counting point. The vehicle split is assumed: 18 % trucks on motorways and trunk roads, 12 % on primary roads, 4 % motorcycles. Milan has counts from the [EU city traffic dataset](https://github.com/XavB64/traffic-volume-data-EU-cities).

Motorways run by concession companies (most of the autostrada network, the A4 at Milan included) are not in the Anas census and follow the model of the trips made around them. Other regional and city roads have no counts. Uncounted streets carry the trips their buildings make. Uncounted main roads follow one model of the trips made around them, fitted on counted roads.

Vehicle mix: where not counted, the medium and heavy vehicles that counted roads of the same class carry; motorcycles 3.8 % of a town street's traffic (WHO 2023); electric cars 0.7 % of cars, rolling noise only (IEA 2024).

Unsigned roads: a primary road 50 km/h in towns, 70 outside, a motorway 130.

## Railways

Italy has no national open timetable. Train counts come from five regional feeds, one busy Wednesday:

| Feed | Region |
|---|---|
| [Trenitalia via Regione Toscana](https://dati.toscana.it/dataset/8bb8f8fe-fe7d-41d0-90dc-49f2456180d1) | Tuscany and trains passing through it |
| [Trenord](https://www.dati.lombardia.it/) | Lombardy |
| [GTT](https://www.gtt.to.it/open_data/) | Turin area |
| Ferrotramviaria (2025 archive) | Bari to Barletta |
| [Trenitalia Sardegna](https://www.sardegnamobilita.it/) | Sardinia |

Lazio, Campania, Veneto and Sicily have no feed. Class defaults apply there: 80 passenger and 20 freight trains per day on main lines, 30 and 5 on branch lines, 120 trams on tram tracks. The timetables are passenger-only. Where no count exists, the class defaults are scaled to the country's train-kilometres (Eurostat 2024); freight comes from them, drawn to the TEN-T corridors, 37 % of it at night as at Germany's railway monitors.

## Buildings and terrain

Terrain from TINITALY.

## Homes and places

Homes mapped in OpenStreetMap: 16 heat pumps and 56 air conditioners per 100 homes. Church bells ring three times a day, with a Sunday peal. Mosques: only those with a mapped minaret call, once on Fridays. Bars without mapped hours open to 02:00 (03:00 at weekends), dinner 20-24 h.

## Checked against

Silenzi in Quota's quiet mountains.
