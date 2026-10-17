---
title: Spain
intro: State road census, three passenger timetables, cadastre floor counts, turbine register for Castilla-La Mancha. Regional roads not covered; rail freight from the national train-kilometres.
---

## Roads

- State network (autopistas, autovías, N roads): [Mapa de Tráfico 2022](https://mapatrafico.transportes.gob.es/2022/) of the transport ministry.
- Madrid, Barcelona, Valencia: [EU city traffic dataset](https://github.com/XavB64/traffic-volume-data-EU-cities).

Regional roads have no counts loaded; those published by Catalonia and Andalucía are not read yet. Uncounted streets carry the trips their buildings make. Uncounted motorways, trunk and primary roads follow one model of the trips made around them, fitted on counted roads, and secondary and tertiary roads a fit of the trips made within 5 km.

Vehicle mix: where not counted, a main road takes the medium and heavy shares of counted roads of its class, a street 1 % medium and 2 % heavy; motorcycles 3 % of a town street's traffic (WHO 2023); electric cars 0.8 % of cars, rolling noise only (IEA 2024); lorries and buses at most 90 km/h.

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

Wind turbines are OpenStreetMap points. A turbine paired with an entry in the [Castilla-La Mancha turbine register](https://datosabiertos.castillalamancha.es/dataset/aerogeneradores) (CC BY-SA), the only open regional register, one to one and nearest first within 500 m, takes its rated power and hub height from it. Elsewhere a turbine keeps the rating OpenStreetMap tags, if any.

The register adds 53 turbines OpenStreetMap lacks.

## Buildings and terrain

Floor counts: [Catastro](https://www.catastro.hacienda.gob.es/INSPIRE/buildings/ES.SDGC.BU.atom.xml) (Dirección General del Catastro), where a cadastre building lies within 30 m of the OpenStreetMap building. The file does not cover every province; elsewhere the height is the typical height for the footprint size.

Terrain from IGN's [MDT05](https://centrodedescargas.cnig.es/) from the PNOA laser survey (© Instituto Geográfico Nacional, CC BY 4.0); the west of the mainland, from Vigo to Badajoz, is not loaded and takes the GEDTM30 world model.

## Homes and places

Homes mapped in OpenStreetMap: 9 heat pumps and 41 air conditioners per 100 homes. Church bells ring three times a day, with a Sunday peal. Mosques: only those with a mapped minaret call, once on Fridays. Bars without mapped hours open to 02:00 (03:00 at weekends), dinner 20-24 h.

## Checked against

The city noise monitors of Barcelona and Madrid ([validation](/about/validation)).

Barcelona noise monitoring station 9907, on a street with a tram. A single modelled tram line exceeded the station's total measured level. Cause: the default tram speed of 40 km/h. Street trams run at about 25 km/h between stops; the default is now 25 km/h everywhere.
