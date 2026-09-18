---
title: Denmark
intro: National and municipal road counts (Mastra), one passenger timetable for all rail, turbine ratings from the Energistyrelsen register. Rail freight not covered.
map: { center: [9.5, 56.0], zoom: 7 }
---

## Roads

Traffic volumes: [Mastra](https://www.opendata.dk/vejdirektoratet/taellinger-nogletal-mastra), the counting system of Vejdirektoratet and the municipalities (yearly average traffic and number of trucks per station). Mastra does not split trucks further; 5 % are assigned to the medium class, the rest to heavy.

Copenhagen also has counts from the [EU city traffic dataset](https://github.com/XavB64/traffic-volume-data-EU-cities). Uncounted streets use the class default, adjusted for surrounding buildings and the counted roads they connect to.

## Railways

Train, S-tog, metro and light rail counts: [Rejseplanen timetable](https://www.rejseplanen.info/labs/GTFS.zip), one busy Wednesday. Freight is not covered: the timetable is passenger-only. Most of the Copenhagen metro runs in tunnels, which emit no noise.

## Industry

Wind turbines are OpenStreetMap points. A turbine within 200 m of an [Energistyrelsen register](https://ens.dk/) entry takes its rated power and hub height from the register.

## Buildings and terrain

The building register BBR has floors and heights for every building; the bulk download requires an account and is not loaded. Heights: OpenStreetMap tags and the GHSL average for the block.
