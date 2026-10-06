# Methodology

Each source gets a sound power per octave band (63 Hz to 8 kHz) for the day (07–19), the evening
(19–23) and the night (23–07). A click sums every source within 12 km of the point (aircraft
within 16 km) at a receiver 4 m above the ground and reports Lden, the three period levels, the
level exceeded 5 % of the time and the loudness. Inside a building the click
answers at the building's loudest façade point, placed as CNOSSOS-EU places receivers for
building exposure; no indoor attenuation is applied.

## Roads

Emission follows CNOSSOS-EU for light, medium and heavy vehicles and two-wheelers: rolling and
propulsion noise, road surface, gradient, acceleration at junctions and the place's yearly air
temperature (WorldClim 2.1). Electric cars, at their country's share (IEA), make rolling noise
only. Motorcycles run 3.8 dB louder than cars at the same speed and 2.9 dB louder pulling away at
junctions (ASJ RTN-Model 2018).

Traffic is counted where a national census, a road department or a city publishes counts. Where
nobody counted, it is estimated, and the click panel marks it as an estimate:

- A local street carries the trips of the buildings it serves. Every building — OpenStreetMap's
  and the Overture footprints OpenStreetMap lacks — makes trip ends that follow the street
  network to the nearest main road; no street carries more than the one it drains into.
- Secondary and tertiary roads follow how the counted roads of 13 European countries relate to
  the trip ends within 5 km, unclassified roads those of Great Britain, France and Sweden within
  2 km, capped where denser cities drive less.
- Motorways, trunk and primary roads across Europe and in the United States, Japan, New Zealand,
  Colombia, Chile and Mexico follow one fit on the counted roads of 19 of these countries: trip
  ends within 1, 5 and 15 km, lanes and one-way. Held out a country at a time it misses by 2.3 dB;
  Mexico, left out of the fit, by 2.9 dB.
- Thailand's national highways carry the highway department's vehicle-kilometres per province,
  its rural roads the rural roads department's counts.

The vehicle mix is counted where counted. Otherwise a road takes the counted median of its class
(16 countries); in the United States its state's mix by road group and area (FHWA Highway
Statistics 2023, table VM-4). Motorcycles come from counts and national fleets (WHO 2023).

Day, evening and night shares are measured by country, road group and vehicle weight: hourly
counts in the United States, Japan, Germany, the Netherlands and Great Britain, roadside monitors
in Spain and Thailand; elsewhere the median of the region's measured countries, or where none is
measured a default between the measured patterns. Within each period the traffic follows
the hourly profile of 242 counters in Baden-Württemberg. Speeds are the posted limit; an untagged
road takes the legal default or, where lower, its class's median signed speed, and a two-way rural
road at a national limit below motorways runs at 0.845 of it (free-flow speeds in Great Britain).

Car parks follow the Bavarian parking study (63 dB(A) per movement, 0.4 movements per space and
hour by day, 0.05 at night). Street parking carries only the cars of the buildings around it.
Roads nobody counted carry the buses of OpenStreetMap's routes.

## Railways

Each category — passenger, high-speed, freight, tram — has one whole-train spectrum with a rolling
term growing as 30 lg v and a constant traction term, shifted to its typical train by CNOSSOS-EU
per-vehicle computations and the pass-bys at Germany's 19 railway noise monitors (2023); a period's
trains spread over the line as in CNOSSOS-EU. Train counts come from GTFS feeds and national
timetables routed along the track network; lines without a timetable take their country's
train-kilometres (Eurostat). Freight without a count is spread over the network by the EU's TEN-T
freight corridors; on European lines 37.6 % of freight trains run between 22:00 and 06:00, as
counted at the same German monitors. Locomotives sound their horn at level crossings in the United
States and Canada.

## Aircraft

A year of recorded ADS-B flights (September 2025 – August 2026) from adsb.lol, with the flights
only ADSBExchange received added from sample days on which both are complete. Each aircraft type
flies its EASA ANP
aircraft (a similar one where the ANP has none) by ECAC Doc 29, 4th edition: altitude above the geoid, thrust from the observed
climb and acceleration, flaps and gear on approach. The flights are summed into boxes of airspace;
at the benchmark's nine points a box reads back within 0.2 dB of every flight summed on its own.
The click lists the ten loudest flights. Taxiing, take-off and landing rolls run on the mapped runways and taxiways. Flights are
missing where no volunteer receiver hears low traffic.

## Industry and ships

Factories, power plants, mines and quarries emit by their type and area; plant types come from
E-PRTR, the Global Power Plant Database and Global Energy Monitor. Wind turbines emit the
published maximum for their rated power less an operating allowance, solar farms from their
inverters by day, substations from their transformers around the clock.

Ships come from AIS vessel density (EMODnet in European seas, Global Fishing Watch elsewhere), as
the mean number of ships present at 108 dB(A) for large ships, 98 for work boats and 88 for leisure
craft.

## Buildings and places

Homes emit the outdoor units their country's households own — heat pumps and air conditioners
per household from national surveys — running the hours their climate asks for (WorldClim degree
days). A Prague home emits 45 dB(A), a Bangkok home 51 dB(A) by day. Shops, schools, hotels and
other buildings emit by their class and floor area.

Church bells ring as events. Europe's Christian churches and bell towers ring the prayer bells
three times a day for three minutes and a Sunday peal; German and Swiss clocks strike the quarters,
Swiss ones at night too. Orthodox churches ring before Saturday's and Sunday's services. A peal
is 114 dB(A), a cathedral's 122.

Mosques call to prayer at their place's prayer times, computed for every mosque with its
country's method and time zone, five calls a day of three minutes at 118 dB(A) from the minaret.
Country rules change this: Saudi Arabia calls at a third of the power and, like Egypt, adds the
iqama; Indonesia adds the recitation before the call, Turkey the Friday sala; Rwanda has no dawn
call; Chinese, Tajik and Singapore mosques do not call outside. In Europe (except Albania, Bosnia
and Herzegovina, Bulgaria, Greece, Kosovo, Montenegro, North Macedonia, Romania, Serbia and
Russia), the Americas, Australia, New Zealand and East Asia only mosques with a mapped minaret
call, once on Fridays at 100 dB(A).

The people outside bars, pubs, nightclubs, beer gardens, restaurants, cafés and fast-food places
sit on their terraces and stand at their doors. A person present emits a talker's sound power with
a third of the people talking: 63.6 dB(A) while eating, 70.7 lively, 73.0 standing at night.
Terrace seats come from the terrace registers of Madrid and Melbourne, occupied 75 % on weekend
evenings and nights and 30 % by day (an assumption: nothing is measured). Bars keep 2.4 people at
the door; clusters of bars add their street crowd at night. Hours are the mapped opening hours,
else the country's legal closing time, else an assumption; terraces count in the warm part of the
year.

Buildings, noise walls and terrain screen sound. Building heights come from measured surveys,
mapped heights, floor counts, Overture heights and finally the typical height of the footprint.

## Propagation

CNOSSOS-EU in eight octave bands: geometric spreading, air absorption (ISO 9613-1) averaged over
the place's 3-hourly weather of 1991–2020, the ground's effect, diffraction over terrain and
buildings, and the share of favourable (downward-refracting) weather by direction from the same
ERA5 climatology. Forest does not attenuate: CNOSSOS-EU has no foliage term. Terrain is a national
elevation model where one is open (mostly LiDAR; Great Britain's OS Terrain 50) and GEDTM30
elsewhere, at one arc-second (about 30 m).

Reflections are not traced. Instead of CNOSSOS-EU's image sources a receiver among buildings gains
3 dB when more than half of nine points 75 m apart around it fall inside buildings taller than
5 m, 1.5 dB when more than a fifth do, else nothing.

## Time and loudness

The level exceeded 5 % of the time comes from each source's passes through the hours of the day.
Loudness in sone follows ISO 532-1, Zwicker's method for steady sound. Each period's received
spectrum — the octave bands shared equally by their thirds, flights by the loudest flight's Doc 29
spectral class — is set to the period's level exceeded 5 % of the time and taken as steady: an
approximation of the N5 that ISO 532-1 defines for time-varying sound. The whole day's loudness is
that of the three periods' spectra averaged over their hours with the evening 5 dB and the night
10 dB up, as in Lden, so it can exceed the loudness of every single period.

## Standards

- [CNOSSOS-EU](https://eur-lex.europa.eu/eli/dir_del/2021/1226): road emission, propagation
- [ECAC Doc 29](https://www.ecac-ceac.org/activities/environment/european-aviation-and-environment-working-group-eaeg/airmod)
  with [EASA ANP](https://www.easa.europa.eu/en/domains/environment/policy-support-and-research/aircraft-noise-and-performance-anp-data)
  data: aircraft
- [ISO 532-1](https://www.iso.org/standard/63077.html): loudness
- [END 2002/49/EC](https://eur-lex.europa.eu/eli/dir/2002/49/oj/eng): Lden and receiver height

The model uses these methods with the inputs above; it is not certified against them. It is an
estimate for comparison and exploration, not a measurement, a legal noise map or a property
certificate.

## Known limits

- Natural sound — wind, water, birds, insects — is not modelled. Where human-made sound is faint,
  a place is louder than the map says.
- Barcelona's and Madrid's wide avenues read 3 to 6 dB too loud.
- Crowds on squares and pedestrians by day are missing: Barcelona's nightlife stations read 6 dB
  under the measurement.
- Night freight on German main lines reads 2.9 dB too quiet.
- OpenStreetMap maps about half of Turkey's mosques and fewer than one in seven in Indonesia,
  Egypt, India and Pakistan; unmapped mosques do not call.
- Deliveries, building sites, garden machinery, music and festivals are not modelled.
