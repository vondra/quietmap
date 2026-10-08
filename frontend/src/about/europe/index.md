---
title: Europe
intro: National road censuses in 11 countries, passenger timetables in 24, street counts in 35 cities. Rail freight from the national train-kilometres.
map: { center: [15, 50], zoom: 4 }
---

## Roads

- National traffic census (motorways and main roads, rarely city streets): Czechia, Denmark, Finland, France, Germany, Great Britain, Ireland, Italy, Norway, Poland, Spain.
- City streets: [EU city traffic dataset](https://github.com/XavB64/traffic-volume-data-EU-cities), 35 cities from Lisbon to Helsinki. A count applies to the OSM way it was published for, or to a road along its line with the same street name or a similar road class. Where a city publishes no truck count, trucks take the class default share; weekday-only counts are scaled to the annual average by 0.9274, the median ratio in cities that publish both. Prague, Brno and Vienna also have their own city counts.

Uncounted streets carry the trips their buildings make. Uncounted motorways, trunk and primary roads follow one model of the trips made within 1, 5 and 15 km, fitted on counted roads — everywhere but Cyprus and Ukraine, whose main roads keep a fixed estimate per class. Secondary and tertiary roads without a count follow a fit of the same kind. Poland's 10,876 noise screens come from its national topographic database (BDOT10k), the Netherlands' from Rijkswaterstaat.

## Railways

24 European countries have a passenger timetable loaded; train counts are those of one busy Wednesday. Great Britain, Romania, Slovenia, Lithuania and most of the Balkans have none.

Lines without a timetable use the class default, in trains per day: main line 80 passenger and 20 freight, branch line 30 and 5, industrial siding 15 freight, tram track 120, light rail 80. In 31 countries these defaults are scaled so that each country's lines carry its official train-kilometres (Eurostat 2024; Great Britain 2019). Lines mapped as passenger-only carry no freight. Track in tunnels emits no noise.

No loaded European timetable contains freight trains: freight comes from the scaled defaults. In the EU it is drawn to the TEN-T corridors — four times the average on a core corridor, twice on a freight line, half off them — and 37.6 % of it runs at night, as counted at Germany's 19 railway noise monitors.

## Industry

Industrial sites are OpenStreetMap areas, classified using the European pollutant register [E-PRTR](https://industry.eea.europa.eu/), the Global Power Plant Database and the Global Energy Monitor lists of steel works, cement plants and coal mines. In 14 countries, most of them in the Balkans, power plants come from the Global Energy Monitor power tracker instead.

Wind turbines take their rated power from a national register in Germany, Denmark, Norway, Sweden, Castilla-La Mancha in Spain, the Netherlands and Belgium; the registers also add the turbines OpenStreetMap lacks and remove those they call dismantled. Turbines without a known rating take 105 dB(A), less an operating allowance.

## Buildings and terrain

Building heights are measured in Prague, North Rhine-Westphalia and the Netherlands. Czechia and Spain add floor counts from national registers. The rest of Europe uses OpenStreetMap tags, Overture and the typical height for the footprint size. Better data for Denmark and Norway is not loaded yet.

Terrain comes from national laser surveys in Austria, Belgium, Czechia, Denmark, France, Germany, Great Britain, Ireland, Italy, the Netherlands, Poland, Portugal, Spain, Sweden and Switzerland; elsewhere from the GEDTM30 world model.

## Homes and places

Homes emit the heat pumps and air conditioners their country's households own. Churches ring their bells three times a day and a Sunday peal; Orthodox churches ring before their services. In most of Europe only mosques with a mapped minaret call, once on Fridays; in Albania, Bosnia and Herzegovina, Bulgaria, Cyprus, Greece, Kosovo, Montenegro, North Macedonia, Romania, Serbia and Russia they call five times a day.

## Ships

European waters: [EMODnet vessel density](https://emodnet.ec.europa.eu/en/human-activities) 2024, built from AIS position reports.

Layer computation is described on the [methodology page](/about/methodology).
