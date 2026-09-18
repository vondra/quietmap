---
title: Australia
intro: Passenger rail timetables for five states. No traffic counts are loaded; all roads use class defaults. Freight rail uses class defaults.
map: { center: [134.0, -25.0], zoom: 4 }
---

## Roads

No traffic counts are loaded; roads use OSM class defaults. Motorways, trunks and primaries use the world default scaled by 0.900 (registered vehicles per road kilometre).

Australia has no national traffic database. Five states publish counts under CC BY 4.0; loaders for them are not yet written.

| State | Source |
|---|---|
| New South Wales | [Transport for NSW open data](https://opendata.transport.nsw.gov.au/) |
| Victoria | [Annual average daily traffic volume](https://discover.data.vic.gov.au/dataset/historical-annual-average-daily-traffic-volume) |
| Queensland | [Traffic census for the state-declared road network](https://www.data.qld.gov.au/dataset/traffic-census-for-the-queensland-state-declared-road-network) |
| Western Australia | [Main Roads traffic digest](https://catalogue.data.wa.gov.au/dataset/mrwa-traffic-digest) |
| South Australia | [Traffic volumes](https://data.sa.gov.au/data/dataset/traffic-volumes) |

## Railways

Passenger trains come from five state timetables.

| State | Feed | What we read |
|---|---|---|
| New South Wales | [Transport for NSW, Greater Sydney](https://opendata.transport.nsw.gov.au/) | Sydney Trains, intercity and regional trains |
| Victoria | [PTV](https://data.ptv.vic.gov.au/downloads/gtfs.zip) | Metro Trains Melbourne, V/Line and interstate trains |
| Queensland | [TransLink South East Queensland](https://gtfsrt.api.translink.com.au/GTFS/SEQ_GTFS.zip) | Brisbane and Gold Coast trains |
| Western Australia | [Transperth](https://www.transperth.wa.gov.au/) | Perth trains |
| South Australia | [Adelaide Metro](https://gtfs.adelaidemetro.com.au/) | Adelaide trains |

Melbourne trams are not read from the Victorian feed; tram tracks use the class default of 120 trams per day. No feed is loaded for Tasmania, the Northern Territory or Canberra.

Freight is not covered: the Pilbara iron ore lines and the Hunter Valley coal lines publish no schedule. Freight defaults to 20 trains per day on a main line and 15 on a line tagged industrial.

## Industry

Power plants: Global Energy Monitor power tracker, operating plants only, supplementing the WRI Global Power Plant Database. Steel plants, cement plants and coal mines: Global Energy Monitor. A plant is a noise source only where OSM maps an industrial area at its location.

Wind turbines are OSM points. The Clean Energy Regulator lists wind farms by postcode only, so missing specs cannot be filled from a register; a turbine without tagged specs is treated as a 2 MW machine.

The National Pollutant Inventory (coordinates and industry codes for reporting facilities) is not used.

## Ships

Global Fishing Watch AIS vessel density.
