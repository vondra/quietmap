---
title: United Kingdom
intro: Department for Transport counts on every road class in Great Britain, street counts in six cities. No rail timetable loaded. Northern Ireland uses class defaults.
map: { center: [-2.5, 54.5], zoom: 6 }
---

## Roads

Traffic volumes: [Department for Transport count points](https://roadtraffic.dft.gov.uk/) (AADF; cars, vans, buses, trucks, motorcycles), most recent year per point. The file covers Great Britain; Northern Ireland has no counts and uses class defaults.

London, Birmingham, Manchester, Glasgow, Edinburgh and Cardiff also have counts from the [EU city traffic dataset](https://github.com/XavB64/traffic-volume-data-EU-cities). Uncounted streets use the class default, adjusted for surrounding buildings and the counted roads they connect to.

## Railways

No timetable is loaded; the national rail timetable requires registration. Class defaults apply: 80 passenger and 20 freight trains per day on main lines, 30 and 5 on branch lines, 120 trams on tram tracks.

## Checked against

Official English noise map, 120 m from the M25: the model was about 13 dB too loud because a cutting and a noise wall were missing from the data. Details under [how we check it](/about/methodology).
