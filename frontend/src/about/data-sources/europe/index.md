---
title: Europe
intro: National road censuses in 15 countries, passenger timetables in 24 (18 from 2024 archives plus 6 declared fallback vintages on the 2024 map), street counts in 34 cities and Amsterdam's traffic model. Rail freight from class defaults, in most countries scaled to the national train-kilometres.
---

<!-- SUBPAGES -->

## Roads

- National traffic census (motorways and main roads, rarely city streets): Czechia, Denmark, Finland, France, Germany, Great Britain, Ireland, Italy, the Netherlands, Norway, Poland, Slovenia, Spain, Sweden, Switzerland.
- City streets: [EU city traffic dataset](https://github.com/XavB64/traffic-volume-data-EU-cities) (CC BY 4.0, with each city's own terms: ODbL for Paris, Grenoble, Rennes and Montpellier, CC BY-SA 4.0 for Brno), 35 cities from Lisbon to Helsinki; Amsterdam's file is the city's traffic model, not counts. A count applies to the OSM way it was published for, or to a road along its line with the same street name or a similar road class. Where a city publishes no truck count, trucks take the median share of the road's class in the cities that publish one; weekday-only counts are scaled to the annual average by 0.9274, the median ratio in cities that publish both. Prague, Brno and Vienna also have their own city counts.

Uncounted streets carry the trips their buildings make. Uncounted motorways, trunk and primary roads follow one model of the trips made within 1, 5 and 15 km, fitted on counted roads — everywhere but Cyprus, and on the 2026 map Russia and Ukraine, whose main roads keep a fixed estimate per class (on the 2024 map, Russia and Ukraine follow the global road model). Secondary and tertiary roads without a count follow a fit of the trips made within 5 km, except in Russia and Ukraine on the 2026 map.

## Railways

On the 2026 map, 24 European countries have a passenger timetable loaded; train counts are those of one busy Wednesday (Wednesday 8 April 2026 for Czechia). On the 2024 map, 18 countries have archived 2024 timetables averaged over every day of 2024, with missing calendar dates estimated from each operator's covered-day mean; because no domestic 2024 archive exists for Bulgaria, Czechia, Denmark, Greece, Croatia or Hungary, their 2025/2026 timetables stand as declared fallback vintages (averaged over their own calendars without asserting zero-service absences, alongside cross-border 2024 archives). Great Britain, Romania, Slovenia, Lithuania, Belarus, Moldova, Ukraine and most of the Balkans have none in either year.

Lines without a timetable use the class default, in trains per day: main line 80 passenger and 20 freight (on the 2026 map: 24.5 freight in Germany, 5.5 in France, 12.0 in Poland, 13.5 in Czechia, 27.1 in Austria, 31.4 in Switzerland), branch line 30 and 5, industrial siding 15 freight, tram track 120, light rail 80; Russia and Ukraine have their own corridor assumptions (2026 table, also applied as a declared fallback vintage on the 2024 map). In 28 European countries on the 2026 map (32 on the 2024 map), the freight defaults are scaled so that each country's lines carry its official goods train-kilometres ([Eurostat](https://ec.europa.eu/eurostat) 2024; Great Britain 2019); on the 2026 map Belgium and Greece publish no 2024 figure and keep the defaults' total, while the 2024 map takes each country's latest published Eurostat year up to 2024 (including Belgium 2011 and Greece 2017). In 12 European countries on the 2026 map (18 on the 2024 map), the passenger defaults exceed the national passenger train-kilometres and are lowered to them. Lines mapped as passenger-only carry no freight. Track in tunnels emits no noise.

No loaded European timetable contains freight trains: freight comes from the defaults. In the EU it is drawn to the TEN-T freight network ([EU TEN-T](https://transport.ec.europa.eu/transport-themes/infrastructure-and-investment/trans-european-transport-network-ten-t_en) maps, GISCO): a line on a core corridor carries twice the freight trains of another TEN-T freight line and eight times those of a line off the network.

Germany's 19 railway noise monitors counted 37.6 % of freight trains between 22:00 and 06:00 in 2023. Spreading the counts evenly within the reported day and night gives 36.8 % between 23:00 and 07:00. The map uses that estimate in the EU, Switzerland, Norway and Great Britain. [Eisenbahn-Bundesamt annual reports](https://www.laerm-monitoring.de/Jahresbericht).

## Industry

Industrial sites are OpenStreetMap areas, classified using the European pollutant register [E-PRTR](https://industry.eea.europa.eu/), the Global Power Plant Database and the Global Energy Monitor lists of steel works, cement plants and coal mines. In 14 countries (Albania, Andorra, Belarus, Bosnia and Herzegovina, Iceland, Kosovo, Liechtenstein, Moldova, Monaco, Montenegro, North Macedonia, San Marino, Serbia and Ukraine) power plants come from the Global Energy Monitor power tracker instead.

Wind turbines take their rated power from a national register in Germany, Denmark, Norway, Sweden, Castilla-La Mancha in Spain, the Netherlands and Belgium; the registers also add the turbines OpenStreetMap lacks and remove those they call dismantled. Turbines without a known rating take 105 dB(A), less an operating allowance.

## Buildings and terrain

Building heights are measured in Prague, North Rhine-Westphalia and the Netherlands. Czechia and Spain add floor counts from national registers. The rest of Europe uses OpenStreetMap tags, Overture and the typical height for the footprint size. Better data for Denmark and Norway is not loaded yet.

Terrain comes from national laser surveys in Austria, Belgium, Czechia, Denmark, France, Germany (without Berlin), Ireland (where flown), the Netherlands, Poland, Portugal (about 11 % of it), Spain (without the west of the mainland), Sweden and Switzerland, and from national models drawn from contours in Great Britain (OS Terrain 50) and Italy (TINITALY); elsewhere from the GEDTM30 world model.

Tree cover comes from [Copernicus Tree Cover Density](https://land.copernicus.eu/en/products/high-resolution-layer-tree-cover-density), and hard ground is refined by [Copernicus Imperviousness Density](https://land.copernicus.eu/en/products/high-resolution-layer-imperviousness).

## Homes and places

Homes emit the heat pumps and air conditioners their country's households own. Churches ring their bells three times a day and a Sunday peal; Orthodox churches ring before their services. In most of Europe only mosques with a mapped minaret call, once on Fridays; in Albania, Bosnia and Herzegovina, Bulgaria, Cyprus, Greece, Kosovo, Montenegro, North Macedonia, Romania, Serbia and Russia they call five times a day.

## Ships

European seas: [EMODnet Human Activities](https://emodnet.ec.europa.eu/en/human-activities) vessel density 2024 (CC BY 4.0), built from AIS position reports. Rivers and inland ports, which EMODnet does not cover, take Global Fishing Watch's AIS vessel presence.

Layer computation is described on the [data preparation page](/about/data-preparation).

<!-- YEAR SOURCES -->
