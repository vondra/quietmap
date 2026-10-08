---
title: Spain
intro: State road census, three passenger timetables, cadastre floor counts, turbine register for Castilla-La Mancha. Regional roads not covered; rail freight from the national train-kilometres.
map: { center: [-3.7, 40.4], zoom: 6 }
---

## Roads

- State network (autopistas, autovías, N roads): [Mapa de Tráfico 2022](https://mapatrafico.transportes.gob.es/2022/) of the transport ministry.
- Madrid, Barcelona, Valencia: [EU city traffic dataset](https://github.com/XavB64/traffic-volume-data-EU-cities).

Regional roads have no counts loaded; those published by Catalonia and Andalucía are not read yet. Uncounted streets carry the trips their buildings make. Uncounted main roads follow one model of the trips made around them, fitted on counted roads.

Vehicle mix: where not counted, the medium and heavy vehicles that counted roads of the same class carry; motorcycles 3 % of a town street's traffic (WHO 2023); electric cars 0.8 % of cars, rolling noise only (IEA 2024); lorries and buses at most 90 km/h.

Unsigned roads: a primary road 30 km/h in towns, 80 outside, a motorway 120.

## Railways

Train counts, one busy Wednesday:

| Feed | What it covers |
|---|---|
| [Renfe AV, LD, MD](https://data.renfe.com/) | High speed, long and medium distance |
| [Renfe Cercanías](https://data.renfe.com/) | Commuter trains |
| [FGC](https://www.fgc.cat/google/google_transit.zip) | Ferrocarrils de la Generalitat de Catalunya |

The northern narrow-gauge lines (former Feve), Euskotren, and metros and trams outside these feeds use the class default. The timetables are passenger-only. Where no count exists, the class defaults are scaled to the country's train-kilometres (Eurostat 2024); freight comes from them, drawn to the TEN-T corridors, 37 % of it at night as at Germany's railway monitors.

## Industry

Wind turbines are OpenStreetMap points. A turbine within 200 m of an entry in the [Castilla-La Mancha turbine register](https://datosabiertos.castillalamancha.es/dataset/aerogeneradores), the only open regional register, takes its rated power and hub height from it. Ratings elsewhere are unknown.

The register adds 53 turbines OpenStreetMap lacks.

## Buildings and terrain

Floor counts: [Catastro](https://www.catastro.hacienda.gob.es/INSPIRE/buildings/ES.SDGC.BU.atom.xml), where a cadastre building lies within 30 m of the OpenStreetMap building. The file does not cover every province; elsewhere the height is the typical height for the footprint size.

Terrain from IGN's MDT05 from the PNOA laser survey.

## Homes and places

Homes mapped in OpenStreetMap: 9 heat pumps and 41 air conditioners per 100 homes. Church bells ring three times a day, with a Sunday peal. Mosques: only those with a mapped minaret call, once on Fridays. Bars without mapped hours open to 02:00 (03:00 at weekends), dinner 20-24 h.

## Checked against

Barcelona noise monitoring station 9907, on a street with a tram. A single modelled tram line exceeded the station's total measured level. Cause: the default tram speed of 40 km/h. Street trams run at about 25 km/h between stops; the default is now 25 km/h everywhere.
