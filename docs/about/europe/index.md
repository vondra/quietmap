---
title: Europe
intro: National road censuses in 11 countries, passenger timetables in 24, street counts in 34 cities. Rail freight not covered.
map: { center: [15, 50], zoom: 4 }
---

## Roads

- National traffic census (motorways and main roads, rarely city streets): Czechia, Denmark, Finland, France, Germany, Great Britain, Ireland, Italy, Norway, Poland, Spain.
- City streets: [EU city traffic dataset](https://github.com/XavB64/traffic-volume-data-EU-cities), 35 cities from Lisbon to Helsinki. A count applies to the OSM way it was published for, or to a road along its line with the same street name or a similar road class. Where a city publishes no truck count, trucks take the class default share; weekday-only counts are scaled to the annual average by 0.93, the median ratio in cities that publish both. Prague, Brno and Vienna also have their own city counts.

Other roads use the [world defaults](/about/methodology).

## Railways

24 European countries have a passenger timetable loaded; train counts are those of one busy Wednesday. Great Britain, Romania, Slovenia, Lithuania and most of the Balkans have none.

Lines without a timetable use the class default, in trains per day: main line 80 passenger and 85 freight, branch line 30 and 5, industrial siding 15 freight, tram track 120, light rail 80. Track in tunnels emits no noise.

Freight is not covered: no loaded European timetable contains freight trains.

## Industry

Industrial sites are OpenStreetMap areas, classified using the European pollutant register [E-PRTR](https://industry.eea.europa.eu/), the Global Power Plant Database and the Global Energy Monitor lists of steel works, cement plants and coal mines. In 14 countries, most of them in the Balkans, power plants come from the Global Energy Monitor power tracker instead.

Wind turbines take their rated power from a national register in Germany, Denmark, Norway, Sweden and Castilla-La Mancha in Spain. Turbines without a known rating are modelled at 105 dB(A) sound power.

## Buildings

Building heights are measured in Prague. Czechia and Spain add floor counts from national registers. The rest of Europe uses OpenStreetMap tags, Overture and the GHSL average height for the block. Better data for the Netherlands, Denmark and Norway is not loaded yet.

## Ships

European waters: [EMODnet vessel density](https://emodnet.ec.europa.eu/en/human-activities) 2024, built from AIS position reports.

Layer computation is described on the [methodology page](/about/methodology).
