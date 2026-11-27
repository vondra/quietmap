---
title: Validation
intro: How close the map comes to real measurements.
---

We compare the map's 2026 outdoor noise levels with microphone readings. Each comparison below
gives the sound sources and hours. The map calculates at 4 m above ground. Microphone heights
vary. The Prague airport microphone positions are approximate.

For aircraft alone in Prague, Minneapolis–St Paul and Zurich, half the readings in each comparison
are within 1.0 to 2.2 dB of the map. In the streets of Paris, Barcelona, Madrid and Dublin, half
are within 2 to 6 dB. In Accra, where the microphones hear speech, music and animals too, the map
reads about 10 dB below them. For scale: twice the traffic on a road adds 3 dB.

A gap is first traced to its cause. It can be the input data (traffic nobody counted, a flight no
receiver heard), a justified difference of method, or a defect of the model. Only a defect leads to
a fix. Each version of the map is checked this way before it goes live. Official strategic noise
maps are cross-checks, not targets the model is tuned to. One exception remains: industry's levels
by sector are calibrated on Czechia's strategic maps of 2022, for want of measured sound powers.

## Map against microphone

Each row compares the model with measurements from one network. **Model minus measured** is the
median difference. In Barcelona the map reads 1.1 dB quieter at the typical microphone.
**Half are within** gives the size of the difference for half the comparisons, whether the map
reads louder or quieter. In Barcelona half are within 4.3 dB. The values are
<abbr title="the day-evening-night level: a year's average, the evening 5 dB and the night 10 dB heavier">Lden</abbr>,
the averages of the hours given, or the US's
<abbr title="the day-night level: a year's average, the night 10 dB heavier">DNL</abbr>;
the current version, November 2026.

| Network | Measurements | Model minus measured | Half are within ± |
|---|---:|---:|---:|
| Paris near its airports: [Bruitparif](https://www.bruitparif.fr), Lden 2024 | 22 | −0.7 dB | 1.7 dB |
| Paris streets: [Ville de Paris](https://opendata.paris.fr/explore/dataset/bruit-evolution-de-l-indice-du-bruit-mesure-sur-des-stations-parisiennes) (ODbL), measured by Bruitparif, Lden 2025 | 16 | −2.3 dB | 2.3 dB |
| Barcelona: [Ajuntament de Barcelona](https://opendata-ajuntament.barcelona.cat/data/dataset/xarxasoroll-equipsmonitor-dades) (CC BY 4.0), Lden 2025 | 154 | −1.1 dB | 4.3 dB |
| Madrid: [Ayuntamiento de Madrid](https://datos.madrid.es/dataset/211356-0-contaminacion-acustica) (CC BY 4.0), Lden 2025 | 31 | +2.3 dB | 3.3 dB |
| Dublin: [Dublin City Council](https://data.smartdublin.ie/sonitus-api) (CC BY), Lden 2025 | 14 | −4.4 dB | 5.7 dB |
| Poland's roads: [Chief Inspectorate of Environmental Protection](https://dane.gov.pl/pl/dataset/1404) (CC BY 4.0), 06–22 h, 2018–2024, at 4,912 locations | 6,191 | +0.2 dB | 3.0 dB |
| Prague airport: [Letiště Praha](https://www.prg.aero/en/noise-air-traffic-and-noise-airport-operations), aircraft only, positions read from the airport map, 06–22 and 22–06 h, January 2026 | 14 and 13 | −0.3 and +1.0 dB | 1.8 and 1.8 dB |
| Minneapolis–St Paul airport: [Metropolitan Airports Commission](https://www.macnoise.com), aircraft only, DNL 2025–26 | 39 | +0.2 dB | 1.0 dB |
| Zurich airport: [Flughafen Zürich](https://report.flughafen-zuerich.ch/2024/ar/en/laerm), aircraft only, 06–22 h, 2024 | 4 | −2.2 dB | 2.2 dB |
| Accra: Clark et al. 2021 ([Zenodo](https://zenodo.org/records/11223686), CC BY 4.0), 24 h, 2019–2020 | 146 | −9.8 dB | 9.8 dB |

The city microphones hear all the sounds around them. For windows such as 06–22 h, the model uses
the corresponding hours of its day, evening and night averages.

The Polish row compares all mapped sources with one reported daytime level per period. In 5,664
of these periods the start and end dates were the same or one day apart. In a further 427 periods
the national file lists several levels without dating the individual readings. The map fell below
their range in 184 periods, within it in 104 and above it in 139. In 286 cases it was inside the
range or within 3 dB of it.

For one of those 427 periods the field report gives each day's reading.
On [Wolności Street in Myszków](/#lat=50.573972&lng=19.30375&z=17&d=50.573972,19.30375&m=lden&calc=1),
a microphone 4 m above ground recorded 62.3 to 66.2 dB by day from 13 to 19 June 2019.
Averaged by sound energy, those seven days gave 65.0 dB. The 2026 map gives 64.4 dB for the same
hours, 06:00–22:00. [GIOŚ field report, table 4](https://www.gov.pl/attachment/8992c366-3f95-4177-8711-6b84daa782c5#page=21).

<details>
<summary>Two nearby monitoring points in Gdańsk</summary>

In 2020 the reported night levels on Słowackiego Street were 53.2 dB at
[number 253](/#lat=54.38515&lng=18.429874&z=18&d=54.38515,18.429874&m=lden&calc=1)
and 69.1 dB at [253D](/#lat=54.385109&lng=18.429993&z=18&d=54.385109,18.429993&m=lden&calc=1).
The published coordinates are 9 m apart. The first falls inside a mapped building, so the map
calculates at its loudest façade, about 1 m from the second point. To estimate 22:00–06:00 we
combine one hour at the map's evening level with seven at its night level. This gives 61.8 and
63.4 dB from all mapped sources, including aircraft. Roads alone give 60.8 and 61.2 dB.
The measurement file omits the height and mounting position of both microphones.
[GIOŚ road measurements, 2020](https://dane.gov.pl/pl/dataset/1404/resource/33678).

</details>

In Zurich, the four points use the mast positions in [Empa's survey, table 12](https://www.aramis.admin.ch/Default?DocumentID=70193&Load=true).
Three fall on buildings, where the map uses a façade. The airport's [December 2024 bulletin](https://media.flughafen-zuerich.ch/-/jssmedia/airport/portal/dokumente/das-unternehmen/politics-and-responsibility/noise-and-sound-insulation/2412_lrmbulletin_dezember.pdf)
shows the sites and lists 9 to 12 months of daytime readings per mast during equipment replacement.

## Loud flights

At Hunters Hill the microphone counted nearly three times as many loud flights as the map
estimated. At St Peters the annual counts were close. The table gives flights per day reaching
70 dB outdoors from September 2025 through August 2026. Open a neighbourhood to see its
**Flights a day** table.

| Sydney neighbourhood | Map | Microphone |
|---|---:|---:|
| [Hunters Hill](/#lat=-33.832806&lng=151.147197&z=15&d=-33.832806,151.147197&popup=%7B%22rest%22%3Atrue%2C%22aircraft%22%3Atrue%7D) | 23 | 67.2 |
| [St Peters](/#lat=-33.912863&lng=151.176991&z=15&d=-33.912863,151.176991&popup=%7B%22rest%22%3Atrue%2C%22aircraft%22%3Atrue%7D) | 107 | 110.6 |
| [Eastlakes](/#lat=-33.9324&lng=151.2084&z=15&d=-33.9324,151.2084&popup=%7B%22rest%22%3Atrue%2C%22aircraft%22%3Atrue%7D) | 78 | 63.1 |

The map counted fewer loud flights at 11 of the 12 Sydney monitors. We included all four
reported categories (arrivals, departures, training and other operations) and divided by all 365 days,
including days with no recorded event. [Airservices Australia’s Sydney neighbourhood map](https://myneighbourhood.emsbk.com/syd5/).

At one Mexico City address the grid estimates 141 loud flights a day. Calculating the peak of
each tracked flight at the address gives 57. [Open the address on the map](/#lat=19.3940679&lng=-99.1378784&z=15&d=19.3940679,-99.1378784&popup=%7B%22rest%22%3Atrue%2C%22aircraft%22%3Atrue%7D).
[How the grid works](/about/noise-at-a-point#what-the-popup-shows).

On a first-floor terrace in Lisbon the median landing peak was 91.9 dB. The modeled peaks
were typically 4.6 dB lower. Of 1,584 matched landings, 1,582 reached 70 dB at the microphone
and all 1,584 did in the model. Of 1,409 matched takeoffs, 1,287 reached 70 dB at the
microphone and 1,406 in the model. The median absolute difference for takeoffs was 3.2 dB.

We matched aircraft identifiers and times on ten dates in July and August 2026. The Type 2
meter used FAST time weighting and sat in a perforated enclosure. The operator reports agreement
within 1–2 dB with two other meters. The reference calibration took place in a new enclosure in October.
[Data and instrument details from Lisbon Finals (CC BY 4.0)](https://www.lisbonfinals.com/research.html).

## Railway noise

Bruitparif could distinguish trains from other sound at 23 of its 26 sites. It analysed one day
per site in November 2019. For the French night (22:00–06:00), the median absolute difference
between those measurements and the map's 2026 annual averages was 4.4 dB.

The two largest differences were on opposite sides of Paris. In
[Villemomble](/#lat=48.87648&lng=2.52&z=16&d=48.87648,2.52&ro=railway&calc=1) the railway noise
works out to 51.2 dB on the map against 64.9 dB measured. At
[Versailles](/#lat=48.80516&lng=2.15998&z=16&d=48.80516,2.15998&ro=railway&calc=1)
it was 57.9 dB on the map against 42.1 dB measured. Both microphones recorded the hours
before 06:00 and after 22:00 on 5 November 2019.

At Villemomble the map estimates 4.7 trains across the two tracks during those hours.
Bruitparif recorded 42 train passages in those hours and identified freight as the main source of night noise.
SNCF supplied an annual average of 30 trains per night for this section.

Open either point and scroll to **Layers** in the detailed calculation. In the **Railways**
row at Versailles the evening level is 62.4 dB and the night level is 56.6 dB. To estimate
the French night we take one hour at the evening level and seven at the night level, averaged by sound energy.
Across the 23 sites the signed median was +1.8 dB. For the four microphones on façades we use
Bruitparif's levels with the 3 dB façade reflection removed.
[Bruitparif field report, results and site photographs](https://www.bruitparif.fr/pages/En-tete/400%20Nos%20publications/700%20Rapports%20d%27%C3%A9tude/2020-06-07%20-%20Rapport%20-%20Campagne%20de%20mesure%20du%20bruit%20ferroviaire%20r%C3%A9alis%C3%A9e%20en%20novembre%202019.pdf#page=18).

Germany's 19 monitors tell us about the trains themselves. Each passing train is measured
7.5 m from its own track, using a different microphone for the other track. We use their train
data in the model. A resident beside the line hears both tracks from one place.
[German railway monitoring method](https://www.laerm-monitoring.de/Messverfahren).

## Quiet places

A place can have little human-made noise and still sound loud. On the
[North Fork Skokomish River Trail](/#lat=47.525142&lng=-123.339283&z=15&d=47.525142,-123.339283&calc=1),
the sources we map average 23.4 dB over 24 hours, calculated from the day, evening and night
levels shown at that point. A microphone recording all sound averaged 51.7 dB in summer 2011.
Park staff heard mostly flowing water. [Skokomish field observations, p. 43](https://irma.nps.gov/DataStore/FileSource/Get/2287637?filename=2021_OLYM_Acoustic_Report.pdf#page=63).

At six sites in Olympic National Park, Washington, the National Park Service ran 13 seasonal
deployments in 2010–11. At five sites every campaign's 24-hour A-weighted average exceeded our
annual estimate for 2026. The microphones heard natural and human sounds together. At Hurricane Ridge the model gave
40.4 dB, higher than even the total sound measured in winter (37.8 dB) and summer (35.5 dB).
The map may overstate transport noise there. The popup attributes it to both roads and aircraft.
[Olympic acoustic report, tables 6 and 28](https://irma.nps.gov/DataStore/Reference/Profile/2287637).

In Europe, the Silenzi in Quota soundwalks recorded all sounds at 73 mountain stops. At the
typical stop the short recording was about 14 dB above the map's annual daytime level. At 14 stops
the map was higher. A road was the loudest modelled source at 13, aircraft at the other.
A short recording can fall between passing vehicles. In the
[woods at Linn of Dee](/#lat=56.990667&lng=-3.544250&z=15&d=56.990667,-3.544250&calc=1)
in Scotland, the recording gave 31.3 dB on 28 May 2023. The map gives 30.5 dB by day at the forest
microphone and 24.1 dB at the peat-bog stop just 206 m away.
[Soundwalk data (CC BY 4.0)](https://zenodo.org/records/20719737) and
[Oberman and colleagues’ field study](https://doi.org/10.1038/s41598-025-08524-y).

Measured a quiet place? [Tell us](mailto:info@quietmap.org): it is the measurement we need most.
