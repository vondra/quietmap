---
title: Europe
intro: National road censuses in 15 countries, passenger timetables in 24, street counts in 34 cities and Amsterdam's traffic model. Rail freight from class defaults, in most countries scaled to the national train-kilometres.
---

<!-- SUBPAGES -->

## Roads

- National traffic census (motorways and main roads, rarely city streets): Czechia, Denmark, Finland, France, Germany, Great Britain, Ireland, Italy, the Netherlands, Norway, Poland, Slovenia, Spain, Sweden, Switzerland.
- City streets: [EU city traffic dataset](https://github.com/XavB64/traffic-volume-data-EU-cities), 35 cities from Lisbon to Helsinki; Amsterdam's file is the city's traffic model, not counts. A count applies to the OSM way it was published for, or to a road along its line with the same street name or a similar road class. Where a city publishes no truck count, trucks take the median share of the road's class in the cities that publish one; weekday-only counts are scaled to the annual average by 0.9274, the median ratio in cities that publish both. Prague, Brno and Vienna also have their own city counts.

Uncounted streets carry the trips their buildings make. Uncounted motorways, trunk and primary roads follow one model of the trips made within 1, 5 and 15 km, fitted on counted roads — everywhere but Cyprus, Russia and Ukraine, whose main roads keep a fixed estimate per class. Secondary and tertiary roads without a count follow a fit of the trips made within 5 km, except in Russia and Ukraine. Poland's 10,876 noise screens come from its national topographic database (BDOT10k), the Netherlands' from Rijkswaterstaat.

## Railways

24 European countries have a passenger timetable loaded; train counts are those of one busy Wednesday. Great Britain, Romania, Slovenia, Lithuania, Belarus, Moldova, Ukraine and most of the Balkans have none.

Lines without a timetable use the class default, in trains per day: main line 80 passenger and 20 freight (24.5 freight in Germany, 5.5 in France, 12.0 in Poland, 13.5 in Czechia, 27.1 in Austria, 31.4 in Switzerland), branch line 30 and 5, industrial siding 15 freight, tram track 120, light rail 80; Russia and Ukraine have their own. In 28 countries the freight defaults are scaled so that each country's lines carry its official goods train-kilometres (Eurostat 2024; Great Britain 2019); Belgium and Greece publish none and keep the defaults' total. In 12 countries the passenger defaults exceed the national passenger train-kilometres and are lowered to them. Lines mapped as passenger-only carry no freight. Track in tunnels emits no noise.

No loaded European timetable contains freight trains: freight comes from the defaults. In the EU it is drawn to the TEN-T freight network: a line on a core corridor carries twice the freight trains of another TEN-T freight line and eight times those of a line off the network. In the EU, Switzerland, Norway and Great Britain 36.8 % of it runs at night (23-07 h), from the counts at Germany's 19 railway noise monitors.

## Industry

Industrial sites are OpenStreetMap areas, classified using the European pollutant register [E-PRTR](https://industry.eea.europa.eu/), the Global Power Plant Database and the Global Energy Monitor lists of steel works, cement plants and coal mines. In 14 countries (Albania, Andorra, Belarus, Bosnia and Herzegovina, Iceland, Kosovo, Liechtenstein, Moldova, Monaco, Montenegro, North Macedonia, San Marino, Serbia and Ukraine) power plants come from the Global Energy Monitor power tracker instead.

Wind turbines take their rated power from a national register in Germany, Denmark, Norway, Sweden, Castilla-La Mancha in Spain, the Netherlands and Belgium; the registers also add the turbines OpenStreetMap lacks and remove those they call dismantled. Turbines without a known rating take 105 dB(A), less an operating allowance.

## Buildings and terrain

Building heights are measured in Prague, North Rhine-Westphalia and the Netherlands. Czechia and Spain add floor counts from national registers. The rest of Europe uses OpenStreetMap tags, Overture and the typical height for the footprint size. Better data for Denmark and Norway is not loaded yet.

Terrain comes from national laser surveys in Austria, Belgium, Czechia, Denmark, France, Germany (without Berlin), Ireland (where flown), the Netherlands, Poland, Portugal (about 11 % of it), Spain (without the west of the mainland), Sweden and Switzerland, and from national models drawn from contours in Great Britain (OS Terrain 50) and Italy (TINITALY); elsewhere from the GEDTM30 world model.

## Homes and places

Homes emit the heat pumps and air conditioners their country's households own. Churches ring their bells three times a day and a Sunday peal; Orthodox churches ring before their services. In most of Europe only mosques with a mapped minaret call, once on Fridays; in Albania, Bosnia and Herzegovina, Bulgaria, Cyprus, Greece, Kosovo, Montenegro, North Macedonia, Romania, Serbia and Russia they call five times a day.

## Ships

European seas: [EMODnet vessel density](https://emodnet.ec.europa.eu/en/human-activities) 2024, built from AIS position reports. Rivers and inland ports, which EMODnet does not cover, take Global Fishing Watch's AIS vessel presence.

Layer computation is described on the [data preparation page](/about/data-preparation).
