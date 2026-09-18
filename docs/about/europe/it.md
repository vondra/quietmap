---
title: Italy
intro: Anas census on motorways and state roads, passenger timetables for five regions. Railways in Lazio, Campania, Veneto and Sicily use class defaults. Rail freight not covered.
map: { center: [12.5, 42.5], zoom: 6 }
---

## Roads

Motorways and state roads: [Anas](https://www.stradeanas.it/) daily traffic census (TGM), one total per counting point. The vehicle split is assumed: 18 % trucks on motorways and trunk roads, 12 % on primary roads, 4 % motorcycles. Milan has counts from the [EU city traffic dataset](https://github.com/XavB64/traffic-volume-data-EU-cities).

Motorways run by concession companies (most of the autostrada network, the A4 at Milan included) are not in the Anas census and use class defaults. Other regional and city roads have no counts. Uncounted streets use the class default, adjusted for surrounding buildings and the counted roads they connect to.

## Railways

Italy has no national open timetable. Train counts come from five regional feeds, one busy Wednesday:

| Feed | Region |
|---|---|
| [Trenitalia via Regione Toscana](https://dati.toscana.it/dataset/8bb8f8fe-fe7d-41d0-90dc-49f2456180d1) | Tuscany and trains passing through it |
| [Trenord](https://www.dati.lombardia.it/) | Lombardy |
| [GTT](https://www.gtt.to.it/open_data/) | Turin area |
| Ferrotramviaria (2025 archive) | Bari to Barletta |
| [Trenitalia Sardegna](https://www.sardegnamobilita.it/) | Sardinia |

Lazio, Campania, Veneto and Sicily have no feed. Class defaults apply there: 80 passenger and 20 freight trains per day on main lines, 30 and 5 on branch lines, 120 trams on tram tracks. Freight is not covered: the timetables are passenger-only.
