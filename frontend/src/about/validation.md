---
title: Validation
intro: How close the map comes to real measurements.
---

We compare the map's 2026 outdoor noise levels with microphone readings. Each comparison uses the
sound sources and hours in the table. The map calculates at 4 m above ground. Microphone heights
vary. For Prague and Germany's railways we use approximate locations.

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

Two numbers for each network. **Model minus measured** is the typical difference: half of the
microphones lie below it and half above. In Barcelona it is −1.1 dB: at the typical microphone the
map is 1.1 dB quieter than measured. **Half are within** is how far the map stands from half of the
microphones, louder or quieter. In Barcelona half are within ±4.3 dB of the map. The values are
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
| Poland's roads: [Chief Inspectorate of Environmental Protection](https://dane.gov.pl/pl/dataset/1404) (CC BY 4.0), 06–22 h, 2018–2024, at 5,351 points | 6,616 | +0.1 dB | 3.0 dB |
| Germany's railways: [Eisenbahn-Bundesamt](https://www.laerm-monitoring.de/Jahresbericht), trains only, approximate trackside positions, 06–22 and 22–06 h, 2023 | 19 | −0.4 and −3.2 dB | 1.7 and 3.3 dB |
| Prague airport: [Letiště Praha](https://www.prg.aero/en/noise-air-traffic-and-noise-airport-operations), aircraft only, positions read from the airport map, 06–22 and 22–06 h, January 2026 | 14 and 13 | −0.3 and +1.0 dB | 1.8 and 1.8 dB |
| Minneapolis–St Paul airport: [Metropolitan Airports Commission](https://www.macnoise.com), aircraft only, DNL 2025–26 | 39 | +0.2 dB | 1.0 dB |
| Zurich airport: [Flughafen Zürich](https://report.flughafen-zuerich.ch/2024/ar/en/laerm), aircraft only, 06–22 h, 2024 | 4 | −2.2 dB | 2.2 dB |
| Accra: Clark et al. 2021 ([Zenodo](https://zenodo.org/records/11223686), CC BY 4.0), 24 h, 2019–2020 | 146 | −9.8 dB | 9.8 dB |

The city microphones hear all the sounds around them. For windows such as 06–22 h, the model uses
the corresponding hours of its day, evening and night averages.

In Zurich, the four points use the mast positions in [Empa's survey, table 12](https://www.aramis.admin.ch/Default?DocumentID=70193&Load=true).
Three fall on buildings, where the map uses a façade. The airport's [December 2024 bulletin](https://media.flughafen-zuerich.ch/-/jssmedia/airport/portal/dokumente/das-unternehmen/politics-and-responsibility/noise-and-sound-insulation/2412_lrmbulletin_dezember.pdf)
shows the sites and lists 9 to 12 months of daytime readings per mast during equipment replacement.

On Germany's railways the map samples 7.5 m from the track near each named town. The distance
along the line to the microphone is uncertain. The same monitoring report supplies the map's
typical train, which limits this comparison as an independent check.

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
