---
title: Validation
intro: How close the map comes to real measurements.
---

The map is computed, so we check it where microphones measured. At each microphone of the
monitoring networks and research campaigns below, the map's level at that exact spot is computed
for the same hours and set beside what the microphone measured. Around three of the four airports
and along Poland's roads and Germany's railways, half of the microphones are within about 1 to 3 dB
of the map; in the streets of Paris, Barcelona, Madrid and Dublin within 3 to 6 dB. In Accra, where
the microphones hear speech, music and animals too, the map reads about 10 dB below them. For
scale: twice the traffic on a road adds 3 dB.

A gap is first traced to its cause: the input data (traffic nobody counted, a flight no receiver
heard), a justified difference of method, or a defect of the model; only a defect leads to a fix.
Each version of the map is checked this way before it goes live. Official strategic noise maps are
cross-checks, not targets the model is tuned to, with one exception still: industry's levels by
sector are calibrated on Czechia's strategic maps of 2022, for want of measured sound powers.

## Map against microphone

Two numbers for each network. **Model minus measured** is the typical difference: half of the
microphones lie below it and half above. In Barcelona it is −1.6 dB: at the typical microphone the
map is 1.6 dB quieter than measured. **Half are within** is how far the map stands from half of the
microphones, louder or quieter: in Barcelona half are within ±4.2 dB of the map. The values are
<abbr title="the day-evening-night level: a year's average, the evening 5 dB and the night 10 dB heavier">Lden</abbr>,
the averages of the hours given, or the US's
<abbr title="the day-night level: a year's average, the night 10 dB heavier">DNL</abbr>;
the current version, October 2026.

| Network | Measurements | Model minus measured | Half are within ± |
|---|---:|---:|---:|
| Paris near its airports: [Bruitparif](https://www.bruitparif.fr), Lden 2024 | 22 | −0.5 dB | 1.5 dB |
| Paris streets: [Ville de Paris](https://opendata.paris.fr/explore/dataset/bruit-evolution-de-l-indice-du-bruit-mesure-sur-des-stations-parisiennes) (ODbL), measured by Bruitparif, Lden 2025 | 16 | −3.2 dB | 3.2 dB |
| Barcelona: [Ajuntament de Barcelona](https://opendata-ajuntament.barcelona.cat/data/dataset/xarxasoroll-equipsmonitor-dades) (CC BY 4.0), Lden 2025 | 154 | −1.6 dB | 4.2 dB |
| Madrid: [Ayuntamiento de Madrid](https://datos.madrid.es/dataset/211356-0-contaminacion-acustica) (CC BY 4.0), Lden 2025 | 31 | +2.4 dB | 3.4 dB |
| Dublin: [Dublin City Council](https://data.smartdublin.ie/sonitus-api) (CC BY), Lden 2025 | 14 | −4.4 dB | 5.5 dB |
| Poland's roads: [Chief Inspectorate of Environmental Protection](https://dane.gov.pl/pl/dataset/1404) (CC BY 4.0), 06–22 h, 2018–2024, at 5,351 points | 6,616 | +0.4 dB | 3.0 dB |
| Germany's railways: [Eisenbahn-Bundesamt](https://www.laerm-monitoring.de), trains only, 06–22 and 22–06 h, 2023 | 19 | −0.4 and −3.2 dB | 1.7 and 3.3 dB |
| Prague airport: [Letiště Praha](https://www.prg.aero), aircraft only, 06–22 and 22–06 h, January 2026 | 14 and 13 | −0.3 and +1.0 dB | 1.8 and 2.0 dB |
| Minneapolis–St Paul airport: [Metropolitan Airports Commission](https://www.macnoise.com), aircraft only, DNL 2025–26 | 39 | +0.2 dB | 1.1 dB |
| Zurich airport: [Flughafen Zürich](https://report.flughafen-zuerich.ch/2024/ar/en/laerm), aircraft only, 06–22 h, 2024 | 4 | −5.9 dB | 5.9 dB |
| Accra: Clark et al. 2021 ([Zenodo](https://zenodo.org/records/11223686), CC BY 4.0), 24 h, 2019–2020 | 146 | −9.6 dB | 9.6 dB |

The city microphones hear everything, the map only its sources. The map's typical train comes from the same German monitoring report, so that row is
not an independent test.

## Quiet places

Nearly all these microphones stand in loud places; the quiet places the map is for are hardly
checked yet. The only set so far is [Silenzi in Quota](https://zenodo.org/records/20719737)'s 73
recordings of a few daytime minutes in quiet mountains (CC BY 4.0). It hears everything, nature
included, so it should read above the map, and it does: by 13.0 dB at the typical point. But at 14
points the map is louder than everything measured, at 13 of them because of a road.

Measured a quiet place? [Tell us](mailto:info@quietmap.org): it is the measurement we need most.
