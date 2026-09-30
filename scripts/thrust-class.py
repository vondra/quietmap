#!/usr/bin/env python3
"""Prints the Rust literals of one noise class's thrust model and anchor profile from the EASA ANP
database v2.3, as dev4's generator (`scripts/build-aircraft-thrust.py`, `build-aircraft-profiles.py`)
wrote them for `thrust_generated.rs` and `profiles_generated.rs`: the Eq. B-1 ratings (MaxTakeoff,
MaxClimb, IdleApproach), the median DEFAULT stage weight, the minimum-R departure flap's R, the
cutback height (the highest end altitude of a DEFAULT climb step at MaxTakeoff), and every SEL
and LAmax NPD row per operation by power (padded to six with the loudest); the anchor profile
holds the lowest approach row and the highest departure row, 160 kt, and the installation given.

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
    out = []
    def rows_block(op):
        sel, lmax = table(op, "SEL"), table(op, "LAmax")
        assert [p for p, _ in sel] == [p for p, _ in lmax]
        count = len(sel)
        pad = lambda seq: seq + [seq[-1]] * (MAX_ROWS - len(seq))
        power = pad([p for p, _ in sel])
        sels = pad([c for _, c in sel])
        lmaxs = pad([c for _, c in lmax])
        return count, power, sels, lmaxs
    dep = rows_block("D")
    app = rows_block("A")
    curve = lambda c: "[" + ", ".join(fmt(v) for v in c) + "]"
    print(f"    // {class_name} <- {acft} (median stage weight, R from clean flap {flap})")
    print("    ThrustModel {")
    print(f'        class_name: "{class_name}",')
    print(f'        anchor_name: "{label}",')
    print("        has_thrust: true,")
    print(f"        engines: {engines},")
    print(f"        weight_lb: {fmt(weight)},")
    print(f"        drag_ratio: {fmt(drag)},")
    print(f"        cutback_ft_afe: {fmt(cutback)},")
    print("        // B-1 coefficients (E, F, Ga, Gb, H) per rating.")
    print(f"        takeoff_coef: {curve(coef('MaxTakeoff'))},")
    print(f"        climb_coef: {curve(coef('MaxClimb'))},")
    print(f"        idle_coef: {curve(coef('IdleApproach'))},")
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
