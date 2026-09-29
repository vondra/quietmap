---
title: Spain
intro: State road census, three passenger timetables, cadastre floor counts, turbine register for Castilla-La Mancha. Regional roads and rail freight not covered.
map: { center: [-3.7, 40.4], zoom: 6 }
---

## Roads

- State network (autopistas, autovías, N roads): [Mapa de Tráfico 2022](https://mapatrafico.transportes.gob.es/2022/) of the transport ministry.
- Madrid, Barcelona, Valencia: [EU city traffic dataset](https://github.com/XavB64/traffic-volume-data-EU-cities).

Regional roads have no counts loaded; those published by Catalonia and Andalucía are not read yet. Uncounted streets use the class default, adjusted for surrounding buildings and the counted roads they connect to.

## Railways

Train counts, one busy Wednesday:

| Feed | What it covers |
|---|---|
| [Renfe AV, LD, MD](https://data.renfe.com/) | High speed, long and medium distance |
| [Renfe Cercanías](https://data.renfe.com/) | Commuter trains |
| [FGC](https://www.fgc.cat/google/google_transit.zip) | Ferrocarrils de la Generalitat de Catalunya |

The northern narrow-gauge lines (former Feve), Euskotren, and metros and trams outside these feeds use the class default. Freight is not covered: the timetables are passenger-only.

## Industry

Wind turbines are OpenStreetMap points. A turbine within 200 m of an entry in the [Castilla-La Mancha turbine register](https://datosabiertos.castillalamancha.es/dataset/aerogeneradores), the only open regional register, takes its rated power and hub height from it. Ratings elsewhere are unknown.

## Buildings and terrain

Floor counts: [Catastro](https://www.catastro.hacienda.gob.es/INSPIRE/buildings/ES.SDGC.BU.atom.xml), where a cadastre building lies within 30 m of the OpenStreetMap building. The file does not cover every province; elsewhere the height is the typical height for the footprint size.

## Checked against

Barcelona noise monitoring station 9907, on a street with a tram. A single modelled tram line exceeded the station's total measured level. Cause: the default tram speed of 40 km/h. Street trams run at about 25 km/h between stops; the default is now 25 km/h everywhere.
