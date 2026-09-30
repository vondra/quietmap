#!/usr/bin/env python3
"""Prints the Rust literals of one noise class's thrust model and anchor profile from the EASA ANP
database v2.3, as dev4's generator (`scripts/build-aircraft-thrust.py`, `build-aircraft-profiles.py`)
wrote them for `thrust_generated.rs` and `profiles_generated.rs`: the Eq. B-1 ratings (MaxTakeoff,
MaxClimb, IdleApproach), the median DEFAULT stage weight, the minimum-R departure flap's R, the
cutback height (the highest end altitude of a DEFAULT climb step at MaxTakeoff), and every SEL
and LAmax NPD row per operation by power (padded to six with the loudest); the anchor profile
holds the lowest approach row and the highest departure row, 160 kt, and the installation given.

A class whose NPD power is a percentage of its maximum sea-level static thrust gets its rows in
pounds; a row repeating the previous row's curves is dropped (the ATR72's 5,310 lb row repeats
its 5,300, its 900 lb row its 890); one without an IdleApproach rating (the ANP rates the DHC830 at MaxTakeoff and MaxClimb
only) gets no idle floor; one the ANP rates by its propeller (CNA172) gets that rating's efficiency
and net propulsive power for Eq. B-5 and zero B-1 coefficients.

usage: thrust-class.py ANP_2_3_DIR CLASS_NAME ACFT_ID LABEL INSTALLATION
"""
import csv, os, statistics, sys

MAX_ROWS = 6


def rows(directory, name):
    with open(os.path.join(directory, name), newline="", encoding="latin1") as handle:
        reader = csv.reader(handle, delimiter=";")
        header = next(reader)
        return [dict(zip(header, [cell.strip() for cell in row])) for row in reader]


def num(text):
    return float(text) if text not in ("", None) else 0.0


def fmt(value):
    text = repr(float(value))
    return text


def main(anp, class_name, acft, label, installation):
    aircraft = next(a for a in rows(anp, "ANP2.3_Aircraft.csv") if a["ACFT_ID"] == acft)
    engines = int(float(aircraft["Number Of Engines"]))
    npd_id = aircraft["NPD_ID"]
    ratings = {r["Thrust Rating"]: r for r in rows(anp, "ANP2.3_Jet_engine_coefficients.csv") if r["ACFT_ID"] == acft}
    coef = lambda rating: [num(ratings[rating][k]) for k in ("E", "F", "Ga", "Gb", "H")]
    percent = "%" in aircraft["Power Parameter"]
    static_lb = float(aircraft["Max Sea Level Static Thrust (lb)"])
    weights = [float(w["Weight (lb)"]) for w in rows(anp, "ANP2.3_Default_weights.csv") if w["ACFT_ID"] == acft]
    weight = statistics.median(weights)
    drags = [(float(a["R"]), a["Flap_ID"]) for a in rows(anp, "ANP2.3_Aerodynamic_coefficients.csv")
             if a["ACFT_ID"] == acft and a["Op Type"] == "D" and a["R"]]
    drag, flap = min(drags)
    steps = [s for s in rows(anp, "ANP2.3_Default_departure_procedural_steps.csv")
             if s["ACFT_ID"] == acft and s["Profile_ID"] == "DEFAULT" and s["Stage Length"] == "1"]
    cutback = max(float(s["End Point Altitude (ft)"]) for s in steps
                  if s["Step Type"] == "Climb" and s["Thrust Rating"] == "MaxTakeoff")
    npd = [n for n in rows(anp, "ANP2.3_NPD_data.csv") if n["NPD_ID"] == npd_id]
    distances = ["L_200ft", "L_400ft", "L_630ft", "L_1000ft", "L_2000ft", "L_4000ft", "L_6300ft", "L_10000ft", "L_16000ft", "L_25000ft"]
    def table(op, metric):
        found = sorted(((float(n["Power Setting"]), [float(n[d]) for d in distances]) for n in npd
                        if n["Op Mode"] == op and n["Noise Metric"] == metric), key=lambda t: t[0])
        return found
    def repeated(op):  # powers whose SEL and LAmax curves repeat the previous row's
        sel, lmax = table(op, "SEL"), table(op, "LAmax")
        return {sel[k][0] for k in range(1, len(sel))
                if sel[k][1] == sel[k - 1][1] and lmax[k][1] == lmax[k - 1][1]}
    out = []
    def rows_block(op):
        drop = repeated(op)
        sel = [row for row in table(op, "SEL") if row[0] not in drop]
        lmax = [row for row in table(op, "LAmax") if row[0] not in drop]
        assert [p for p, _ in sel] == [p for p, _ in lmax]
        count = len(sel)
        pad = lambda seq: seq + [seq[-1]] * (MAX_ROWS - len(seq))
        to_pounds = (lambda p: round(p * static_lb / 100.0, 2)) if percent else (lambda p: p)
        power = pad([to_pounds(p) for p, _ in sel])
        sels = pad([c for _, c in sel])
        lmaxs = pad([c for _, c in lmax])
        return count, power, sels, lmaxs
    dep = rows_block("D")
    app = rows_block("A")
    curve = lambda c: "[" + ", ".join(fmt(v) for v in c) + "]"
    unit = f", rows at % of {static_lb:.0f} lb" if percent else ""
    print(f"    // {class_name} <- {acft} (median stage weight, R from clean flap {flap}{unit})")
    print("    ThrustModel {")
    print(f'        class_name: "{class_name}",')
    print(f'        anchor_name: "{label}",')
    print("        has_thrust: true,")
    print(f"        engines: {engines},")
    print(f"        weight_lb: {fmt(weight)},")
    print(f"        drag_ratio: {fmt(drag)},")
    print(f"        cutback_ft_afe: {fmt(cutback)},")
    print("        // B-1 coefficients (E, F, Ga, Gb, H) per rating.")
    propellers = {r["Thrust Rating"]: [float(r["Propeller Efficiency"]), float(r["Installed Net Propulsive Power (hp)"])]
                  for r in rows(anp, "ANP2.3_Propeller_engine_coefficients.csv") if r["ACFT_ID"] == acft}
    rating = lambda name: coef(name) if name in ratings else [0.0] * 5
    print(f"        takeoff_coef: {curve(rating('MaxTakeoff'))},")
    print(f"        climb_coef: {curve(rating('MaxClimb'))},")
    print(f"        idle_coef: {curve(rating('IdleApproach'))},")
    propeller = [propellers.get(name, [0.0, 0.0]) for name in ("MaxTakeoff", "MaxClimb")]
    print(f"        propeller: [{curve(propeller[0])}, {curve(propeller[1])}],")
    for name, (count, power, sels, lmaxs) in (("dep", dep), ("app", app)):
        print(f"        {name}_rows: {count},")
        print(f"        {name}_power: {curve(power)},")
        print(f"        {name}_sel: [")
        for c in sels:
            print(f"            {curve(c)},")
        print("        ],")
        print(f"        {name}_lmax: [")
        for c in lmaxs:
            print(f"            {curve(c)},")
        print("        ],")
    print("    },")
    print("---- profile")
    print("    NpdProfile::new(")
    print(f'        "{label}",')
    print(f"        {curve(app[2][0])},")
    print(f"        {curve(dep[2][dep[0]-1])},")
    print(f"        {curve(app[3][0])},")
    print(f"        {curve(dep[3][dep[0]-1])},")
    print("        160.0,")
    print(f"        Installation::{installation},")
    print("    ),")


if __name__ == "__main__":
    main(*sys.argv[1:6])
