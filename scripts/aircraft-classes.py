#!/usr/bin/env python3
"""Writes the aircraft noise classes from the EASA ANP database (v2.3 tables, v9 workbooks; v9 wins
where both hold an aircraft): one class per ANP aircraft that a designator reads, as Doc 29 reads
each aircraft's own data, plus the fallback proxy (class 0) and the helicopters, both pinned.

dev4 collapsed its 124 profiles onto 15 classes by the likeness of their loudest curves, so an
aircraft flew another's weight, ratings and rows (an E175 an A320's, an A330-200 a 737-800's, an
A300-600 freighter a 737-800's), and mapped several designators to the wrong ANP aircraft (B737,
the 737-700, to the 737-300; the ATR 72 to the Dash 8). The class of a designator is now its ANP
aircraft (`MAPPING`), the proxy where the ANP has none noted there.

Per class: the Eq. B-1 ratings MaxTakeoff, MaxClimb and IdleApproach (zero where the ANP tabulates
none: the lowest row bounds it) or, for an aircraft the ANP rates by its propeller, Eq. B-5's
efficiency and power; the median DEFAULT stage weight; the minimum-R departure flap's R; the
cutback height (the end altitude of the last MaxTakeoff step before the first MaxClimb step); every
SEL and LAmax NPD row by power, in pounds (a row in % of the maximum static thrust moved to
pounds; a row repeating the previous row's curves dropped); the final approach configuration of
Doc 29 B11 (the landing flap of the DEFAULT approach, its R, the height the approach flies it
from, 90 % of the maximum landing weight; without ANP approach steps the draggiest approach flap,
from the height a sibling flies it); the spectral classes (Appendix D); the family (the runway
rolls and Stage 1's speed cap); the installation. Each designator's profile holds its aircraft's
lowest approach and highest departure rows at 160 kt.

Rewrites in physics/src/doc29: profiles_generated.rs (its class tables and its ANP profiles),
thrust_generated.rs, approach_generated.rs, spectra_generated.rs.

usage: aircraft-classes.py ANP_2_3_DIR ANP_V9_DIR REPO_ROOT  (needs openpyxl)
"""
import csv, os, re, statistics, sys
import openpyxl

MAX_ROWS = 6
DISTANCES = ["L_200ft", "L_400ft", "L_630ft", "L_1000ft", "L_2000ft", "L_4000ft", "L_6300ft",
             "L_10000ft", "L_16000ft", "L_25000ft"]

# Designator -> ANP aircraft. Proxies (no ANP entry of their own) are noted.
MAPPING = {
    "B738": "737800", "B739": "737800",  # 737-900: proxy
    "B737": "737700", "B736": "737700",  # 737-600: proxy (the NG with -7B engines)
    "B734": "737400", "B735": "737500", "B733": "7373B2",
    "B38M": "7378MAX", "B39M": "7378MAX", "B37M": "7378MAX",  # MAX 9, MAX 7: proxies
    "A320": "A320-232", "A20N": "A320-270N", "A319": "A319-131",
    "A19N": "A320-270N", "BCS3": "A320-270N", "BCS1": "A320-270N",  # proxies: geared fans
    "A321": "A321-232", "A21N": "A321-270N",
    "B752": "757PW", "B753": "757PW",  # 757-300: proxy
    "B772": "777200", "B77L": "777200", "B773": "777300",  # 777-200LR: proxy
    "B77W": "7773ER", "B77F": "7773ER",  # 777F: proxy (its GE90-110B)
    "B788": "7878R", "B789": "7879", "B78X": "7879",  # 787-10: proxy
    "A332": "A330-301", "A333": "A330-343",
    "A338": "A330-941", "A339": "A330-941",  # A330-800: proxy (its Trent 7000)
    "A359": "A350-941", "A35K": "A350-1041",
    "A306": "A300-622R", "A310": "A310-304",
    "B763": "7673ER", "B764": "767400",
    "MD11": "MD11GE", "DC10": "MD11GE",  # DC-10: proxy (the ANP's DC1030 has no steps)
    "L101": "L1011",
    "B744": "747400", "B748": "7478",
    "B741": "747200", "B742": "747200",  # 747-100: proxy (the ANP's has no ratings)
    "IL76": "747200",  # proxy: a heavy four-engine wing
    "A342": "A340-211", "A343": "A340-211", "A346": "A340-642", "A388": "A380-841",
    "E170": "EMB170", "E75L": "EMB175", "E75S": "EMB175", "E190": "EMB190", "E195": "EMB195",
    "E290": "ERJ190-300", "E295": "ERJ190-400",
    "CRJ2": "CL601",  # proxy: the CF34-3 of the CRJ200
    "CRJ7": "CRJ9-ER", "CRJ9": "CRJ9-ER",  # CRJ700: proxy
    "EMJ": "EMB145", "F70": "F10062",  # Fokker 70: proxy (the Fokker 100's Tay 620)
    "CL60": "CL601",
    # Large business jets without their own ANP ratings: the CRJ900's weight and thrust.
    "GLEX": "CRJ9-ER", "GLF6": "CRJ9-ER", "GLF5": "CRJ9-ER", "FA7X": "CRJ9-ER",
    "C56X": "CIT3", "C680": "CIT3", "PC24": "CIT3",  # proxies
    "LJ60": "LEAR35",  # proxy
    "AT72": "ATR72", "AT76": "ATR72", "AT43": "ATR72", "AT45": "ATR72",  # all but AT72: proxies
    "DH8D": "DHC830", "DH8C": "DHC830", "F50": "DHC830",  # Q400, Fokker 50: proxies
    "DH8A": "DHC8", "DH8B": "DHC8", "JS41": "DHC8",  # Jetstream 41: proxy
    "SF34": "SF340", "L410": "DHC6", "EN48": "DHC6",  # proxies but the Saab
    "C172": "CNA172", "C152": "CNA172", "C182": "CNA172", "PA28": "CNA172", "SR20": "CNA172",
    "SR22": "CNA172", "DA40": "CNA172", "P28A": "CNA172", "C210": "CNA172", "BE36": "CNA172",
    "M20P": "CNA172", "C206": "CNA172", "PA32": "CNA172", "RV7": "CNA172", "RV8": "CNA172",
    "PA34": "PA30", "PA44": "PA30",  # light piston twins: the Twin Comanche
    "DA42": "CNA172",  # a diesel twin certified about as loud as a Cessna 172, 7 dB under the PA-30
}
# The approach of an aircraft without ANP approach steps starts where its sibling's does.
APPROACH_SIBLING = {"777200": "7773ER", "777300": "7773ER", "767400": "7673ER"}


def rows_csv(directory, name):
    with open(os.path.join(directory, name), newline="", encoding="latin1") as handle:
        reader = csv.reader(handle, delimiter=";")
        header = [cell.strip() for cell in next(reader)]
        return [dict(zip(header, [cell.strip() for cell in row])) for row in reader]


def rows_xlsx(directory, name):
    sheet = openpyxl.load_workbook(os.path.join(directory, name), read_only=True).worksheets[0]
    rows = sheet.iter_rows(values_only=True)
    header = [str(cell).strip() if cell is not None else "" for cell in next(rows)]
    return [dict(zip(header, ["" if cell is None else str(cell).strip() for cell in row]))
            for row in rows if row and row[0] not in (None, "")]


def num(text):
    return float(text) if text not in ("", None) else 0.0


class Anp:
    """The ANP tables, v9 rows replacing v2.3 rows of the same aircraft (or NPD id)."""

    def __init__(self, anp, v9):
        def merged(key, old, new):
            ids = {row[key] for row in new}
            return [row for row in old if row[key] not in ids] + new
        v9name = lambda part: f"EASA_ANP_database_{part}_v9.xlsx"
        self.aircraft = {r["ACFT_ID"]: r for r in merged(
            "ACFT_ID", rows_csv(anp, "ANP2.3_Aircraft.csv"), rows_xlsx(v9, v9name("Aircraft")))}
        self.npd = merged("NPD_ID", rows_csv(anp, "ANP2.3_NPD_data.csv"), rows_xlsx(v9, v9name("NPD_Data")))
        self.jet = merged("ACFT_ID", rows_csv(anp, "ANP2.3_Jet_engine_coefficients.csv"),
                          rows_xlsx(v9, v9name("Jet_Engine_Coefficients")))
        self.propeller = rows_csv(anp, "ANP2.3_Propeller_engine_coefficients.csv")
        self.weights = merged("ACFT_ID", rows_csv(anp, "ANP2.3_Default_weights.csv"),
                              rows_xlsx(v9, v9name("Default_Weights")))
        self.aero = merged("ACFT_ID", rows_csv(anp, "ANP2.3_Aerodynamic_coefficients.csv"),
                           rows_xlsx(v9, v9name("Aerodynamic_Coefficients")))
        self.departure = merged("ACFT_ID", rows_csv(anp, "ANP2.3_Default_departure_procedural_steps.csv"),
                                rows_xlsx(v9, v9name("Default_Departure_Procedural_Steps")))
        self.approach = merged("ACFT_ID", rows_csv(anp, "ANP2.3_Default_approach_procedural_steps.csv"),
                               rows_xlsx(v9, v9name("Default_Approach_Procedural_Steps")))
        self.spectra = {r["Spectral Class ID"]: r for r in rows_xlsx(v9, v9name("Spectral_Classes"))}


def fmt(value):
    return repr(float(value))


def curve(values):
    return "[" + ", ".join(fmt(v) for v in values) + "]"


class Aircraft:
    """One ANP aircraft's class data."""

    def __init__(self, anp, acft):
        a = anp.aircraft[acft]
        self.acft = acft
        self.engine_type = a["Engine Type"]
        self.engines = int(float(a["Number Of Engines"]))
        self.mtow_lb = float(a["Max Gross Takeoff Weight (lb)"])
        self.mlw_lb = float(a["Max Gross Landing Weight (lb)"])
        lateral = a.get("Lateral Directivity Identifier") or a.get("Wing") or ""
        self.installation = {"Wing": "Wing", "Fuselage": "Fuselage", "Prop": "Propeller"}[lateral]
        percent = "%" in a["Power Parameter"]
        static_lb = float(a["Max Sea Level Static Thrust (lb)"])
        ratings = {r["Thrust Rating"]: r for r in anp.jet if r["ACFT_ID"] == acft}
        coef = lambda name: [num(ratings[name].get(k)) for k in ("E", "F", "Ga", "Gb", "H")] \
            if name in ratings else [0.0] * 5
        self.takeoff, self.climb, self.idle = coef("MaxTakeoff"), coef("MaxClimb"), coef("IdleApproach")
        props = {r["Thrust Rating"]: [float(r["Propeller Efficiency"]),
                                      float(r["Installed Net Propulsive Power (hp)"])]
                 for r in anp.propeller if r["ACFT_ID"] == acft}
        self.propeller = [props.get(name, [0.0, 0.0]) for name in ("MaxTakeoff", "MaxClimb")]
        assert (self.takeoff[0] > 0 and self.climb[0] > 0) or self.propeller[1][1] > 0, (acft, "no ratings")
        weights = [float(w["Weight (lb)"]) for w in anp.weights
                   if w["ACFT_ID"] == acft and str(w["Stage Length"]).replace(".0", "").isdigit()]
        self.weight_lb = statistics.median(weights)
        drags = [(float(r["R"]), r["Flap_ID"]) for r in anp.aero
                 if r["ACFT_ID"] == acft and r["Op Type"] == "D" and r["R"]]
        self.drag, self.flap = min(drags)
        steps = [s for s in anp.departure if s["ACFT_ID"] == acft
                 and str(s["Stage Length"]).replace(".0", "") == "1"
                 and s["Profile_ID"] in ("DEFAULT", "ICAO_A")]
        if any(s["Profile_ID"] == "DEFAULT" for s in steps):
            steps = [s for s in steps if s["Profile_ID"] == "DEFAULT"]
        steps.sort(key=lambda s: float(s["Step Number"]))
        self.cutback = None
        for k, step in enumerate(steps):
            if step["Thrust Rating"] == "MaxClimb":
                ends = [s for s in steps[:k] if s["Thrust Rating"] == "MaxTakeoff"
                        and s.get("End Point Altitude (ft)")]
                if ends:
                    self.cutback = float(ends[-1]["End Point Altitude (ft)"])
                break
        assert self.cutback is not None and 500.0 <= self.cutback <= 5000.0, (acft, self.cutback)
        npd_id = a["NPD_ID"]
        npd = [n for n in anp.npd if n["NPD_ID"] == npd_id]

        def table(op, metric):
            # Levels to 0.1 dB, as the ANP publishes them (the v9 A321-270N rows carry float noise).
            return sorted(((float(n["Power Setting"]), [round(float(n[d]), 1) for d in DISTANCES])
                           for n in npd if n["Op Mode"] == op and n["Noise Metric"] == metric),
                          key=lambda t: t[0])

        def rows(op):
            sel, lmax = table(op, "SEL"), table(op, "LAmax")
            assert [p for p, _ in sel] == [p for p, _ in lmax], (acft, op)
            keep = [0] + [k for k in range(1, len(sel))
                          if (sel[k][1], lmax[k][1]) != (sel[k - 1][1], lmax[k - 1][1])]
            assert 2 <= len(keep) <= MAX_ROWS, (acft, op, len(keep))
            to_pounds = (lambda p: round(p * static_lb / 100.0, 2)) if percent else (lambda p: p)
            pad = lambda seq: seq + [seq[-1]] * (MAX_ROWS - len(seq))
            return (len(keep), pad([to_pounds(sel[k][0]) for k in keep]),
                    pad([sel[k][1] for k in keep]), pad([lmax[k][1] for k in keep]))
        self.dep, self.app = rows("D"), rows("A")
        self.unit = f", rows at % of {static_lb:.0f} lb" if percent else ""
        own = [s for s in anp.approach if s["ACFT_ID"] == acft]
        start_key = lambda s: s.get("Start Altitude(ft)") or s.get("Start Altitude (ft)")
        a_flaps = {r["Flap_ID"]: float(r["R"]) for r in anp.aero
                   if r["ACFT_ID"] == acft and r["Op Type"] == "A" and r["R"]}
        if own:
            profile = sorted({s["Profile_ID"] for s in own})[0]
            own = sorted((s for s in own if s["Profile_ID"] == profile), key=lambda s: float(s["Step Number"]))
            flap = next(s["Flap_ID"] for s in own if s["Step Type"] == "Land")
            start = next(float(start_key(s)) for s in own if s["Step Type"] == "Descend" and s["Flap_ID"] == flap)
            self.approach = (f"{acft}/{profile}", flap, a_flaps[flap], start)
        elif a_flaps and acft in APPROACH_SIBLING:
            flap = max(a_flaps, key=a_flaps.get)
            sibling = sorted((s for s in anp.approach if s["ACFT_ID"] == APPROACH_SIBLING[acft]),
                             key=lambda s: float(s["Step Number"]))
            land = next(s["Flap_ID"] for s in sibling if s["Step Type"] == "Land")
            start = next(float(start_key(s)) for s in sibling if s["Step Type"] == "Descend" and s["Flap_ID"] == land)
            self.approach = (f"{acft}/draggiest flap, from {APPROACH_SIBLING[acft]}", flap, a_flaps[flap], start)
        else:
            self.approach = None
        self.spectra = (str(int(float(a["Approach Spectral Class ID"]))),
                        str(int(float(a["Departure Spectral Class ID"]))))
        if self.engine_type == "Piston":
            self.family = "Piston"
        elif self.engine_type == "Turboprop":
            self.family = "Turboprop"
        elif self.mtow_lb >= 300_000:
            self.family = "Widebody"
        elif self.mtow_lb >= 120_000:
            self.family = "Narrowbody"
        elif self.mtow_lb >= 55_000:
            self.family = "Regional"
        else:
            self.family = "Business"

    def thrust(self, class_name, label):
        lines = [f"    // {class_name} <- {self.acft} (median stage weight, R from clean flap {self.flap}{self.unit})",
                 "    ThrustModel {",
                 f'        class_name: "{class_name}",',
                 f'        anchor_name: "{label}",',
                 "        has_thrust: true,",
                 f"        engines: {self.engines},",
                 f"        weight_lb: {fmt(self.weight_lb)},",
                 f"        drag_ratio: {fmt(self.drag)},",
                 f"        cutback_ft_afe: {fmt(self.cutback)},",
                 "        // B-1 coefficients (E, F, Ga, Gb, H) per rating.",
                 f"        takeoff_coef: {curve(self.takeoff)},",
                 f"        climb_coef: {curve(self.climb)},",
                 f"        idle_coef: {curve(self.idle)},",
                 f"        propeller: [{curve(self.propeller[0])}, {curve(self.propeller[1])}],"]
        for name, (count, power, sels, lmaxs) in (("dep", self.dep), ("app", self.app)):
            lines.append(f"        {name}_rows: {count},")
            lines.append(f"        {name}_power: {curve(power)},")
            lines.append(f"        {name}_sel: [")
            lines += [f"            {curve(c)}," for c in sels]
            lines.append("        ],")
            lines.append(f"        {name}_lmax: [")
            lines += [f"            {curve(c)}," for c in lmaxs]
            lines.append("        ],")
        lines.append("    },")
        return "\n".join(lines)

    def profile(self, name):
        app_sel, dep_sel = self.app[2][0], self.dep[2][self.dep[0] - 1]
        app_lmax, dep_lmax = self.app[3][0], self.dep[3][self.dep[0] - 1]
        return "\n".join([
            "    NpdProfile::new(",
            f'        "{name}",',
            f"        {curve(app_sel)},",
            f"        {curve(dep_sel)},",
            f"        {curve(app_lmax)},",
            f"        {curve(dep_lmax)},",
            "        160.0,",
            f"        Installation::{self.installation},",
            "    ),",
        ])


def replace_block(text, start_marker, end_marker, new):
    start = text.index(start_marker)
    end = text.index(end_marker, start) + len(end_marker)
    return text[:start] + new + text[end:]


def main(anp_dir, v9_dir, root):
    anp = Anp(anp_dir, v9_dir)
    doc29 = os.path.join(root, "physics/src/doc29")
    profiles_path = os.path.join(doc29, "profiles_generated.rs")
    text = open(profiles_path).read()
    entries = list(re.finditer(r'    NpdProfile::new\(\n        "([^"]+)",\n.*?\n    \),\n', text, re.S))
    names = [m.group(1) for m in entries]
    old_classes = re.search(r"CLASS_OF_PROFILE: \[u8; NUM_PROFILES\] = \[(.*?)\];", text, re.S).group(1)
    old_classes = [int(x) for x in re.findall(r"^\s*(\d+),", old_classes, re.M)]
    old_names = re.findall(r'"([^"]+)"', re.search(r"CLASS_NAMES: \[&str; NUM_CLASSES\] = \[(.*?)\];", text, re.S).group(1))
    designators = [name.split("/")[0] for name in names]
    helicopter = [old_names[c] == "HELICOPTER" for c in old_classes]
    # Classes: the fallback proxy, one per ANP aircraft in profile order, the helicopters.
    anp_ids = []
    for designator, heli in zip(designators, helicopter):
        if designator == "FALLBACK" or heli:
            continue
        acft = MAPPING[designator]
        if acft not in anp_ids:
            anp_ids.append(acft)
    aircraft = {acft: Aircraft(anp, acft) for acft in anp_ids}
    class_names = ["WING_FALLBACK"] + anp_ids + ["HELICOPTER"]
    class_of = []
    for designator, heli in zip(designators, helicopter):
        if designator == "FALLBACK":
            class_of.append(0)
        elif heli:
            class_of.append(len(class_names) - 1)
        else:
            class_of.append(1 + anp_ids.index(MAPPING[designator]))
    # A class's anchor: its first profile (its members read the same curves), the helicopters'
    # the AS50 their calibration is read against.
    anchors = [designators.index("AS50") if name == "HELICOPTER"
               else class_of.index(c) for c, name in enumerate(class_names)]
    # Profiles: every ANP-mapped designator reads its aircraft's curves at 160 kt.
    for m in reversed(entries):
        designator = m.group(1).split("/")[0]
        p = designators.index(designator)
        if class_of[p] in (0, len(class_names) - 1):
            continue
        acft = MAPPING[designator]
        text = text[:m.start()] + aircraft[acft].profile(f"{designator}/{acft}") + "\n" + text[m.end():]
    names = [f"{d}/{MAPPING[d]}" if class_of[p] not in (0, len(class_names) - 1) else names[p]
             for p, d in enumerate(designators)]
    family = ["Narrowbody"] + [aircraft[a].family for a in anp_ids] + ["Helicopter"]
    is_jet = [True] + [aircraft[a].engine_type == "Jet" for a in anp_ids] + [False]
    text = re.sub(r"pub const NUM_CLASSES: usize = \d+;", f"pub const NUM_CLASSES: usize = {len(class_names)};", text)
    text = replace_block(text, "pub static CLASS_NAMES: [&str; NUM_CLASSES] = [", "];",
                         "pub static CLASS_NAMES: [&str; NUM_CLASSES] = [\n"
                         + "".join(f'    "{n}",\n' for n in class_names) + "];")
    text = replace_block(text, "pub static IS_JET: [bool; NUM_CLASSES] = [", "];",
                         "pub static IS_JET: [bool; NUM_CLASSES] = [\n"
                         + "".join(f"    {str(j).lower()}, // {n}\n" for j, n in zip(is_jet, class_names)) + "];")
    family_block = ("pub static CLASS_FAMILY: [Family; NUM_CLASSES] = [\n"
                    + "".join(f"    Family::{f}, // {n}\n" for f, n in zip(family, class_names)) + "];")
    if "pub static CLASS_FAMILY" in text:
        text = replace_block(text, "pub static CLASS_FAMILY: [Family; NUM_CLASSES] = [", "];", family_block)
    else:
        text = text.replace("/// Per-profile → noise class lookup",
                            "/// Per-class family: the runway rolls and Stage 1's speed cap.\n"
                            + family_block + "\n\n/// Per-profile → noise class lookup", 1)
    text = replace_block(text, "pub static CLASS_OF_PROFILE: [u8; NUM_PROFILES] = [", "];",
                         "pub static CLASS_OF_PROFILE: [u8; NUM_PROFILES] = [\n"
                         + "".join(f"    {c}, // {d} → {class_names[c]}\n" for c, d in zip(class_of, designators)) + "];")
    text = replace_block(text, "pub static CLASS_REP_PROFILE_IDX: [u8; NUM_CLASSES] = [", "];",
                         "pub static CLASS_REP_PROFILE_IDX: [u8; NUM_CLASSES] = [\n"
                         + "".join(f"    {p}, // {n} → {designators[p]}\n" for p, n in zip(anchors, class_names)) + "];")
    text = text.replace("use super::npd::{Installation, NpdProfile};", "use super::npd::{Family, Installation, NpdProfile};")
    open(profiles_path, "w").write(text)

    thrust_path = os.path.join(doc29, "thrust_generated.rs")
    header = open(thrust_path).read().split("pub static THRUST")[0]
    models = ['    ThrustModel::pinned("WING_FALLBACK", "FALLBACK/737800"),']
    models += [aircraft[a].thrust(a, names[anchors[1 + k]]) for k, a in enumerate(anp_ids)]
    models.append(f'    ThrustModel::pinned("HELICOPTER", "{names[anchors[-1]]}"),')
    open(thrust_path, "w").write(header + "pub static THRUST: [ThrustModel; NUM_CLASSES] = [\n"
                                  + "\n".join(models) + "\n];\n")

    approach = ['    ApproachConfiguration::none("WING_FALLBACK"),']
    for a in anp_ids:
        config = aircraft[a].approach
        if config is None:
            approach.append(f'    ApproachConfiguration::none("{a}"),')
            continue
        label, flap, drag, start = config
        approach.append(f'    ApproachConfiguration {{ class_name: "{a}", anchor: "{label}", flap: "{flap}", '
                        f"drag_ratio: {drag}, from_ft_afe: {start}, "
                        f"landing_weight_lb: {round(0.9 * aircraft[a].mlw_lb, 1)} }},")
    approach.append('    ApproachConfiguration::none("HELICOPTER"),')
    open(os.path.join(doc29, "approach_generated.rs"), "w").write(
        """//! Generated by `scripts/aircraft-classes.py` from the EASA ANP database v2.3 and v9 (Doc 29
//! 4th ed. Vol 2 B11): per noise class, the landing flap of its aircraft's DEFAULT approach, that
//! flap's drag-to-lift ratio R with the gear down, the height above the field from which the
//! approach flies it, and 90 % of the maximum gross landing weight; without ANP approach steps the
//! draggiest approach flap from the height its sibling flies its own. Not edited by hand.

use super::profiles_generated::NUM_CLASSES;
use super::thrust::ApproachConfiguration;

/// Per-noise-class final approach configuration, in CLASS_NAMES order.
pub static APPROACH: [ApproachConfiguration; NUM_CLASSES] = [
""" + "\n".join(approach) + "\n];\n")

    bands = [k for k in next(iter(anp.spectra.values())) if k.startswith("L_")]
    assert len(bands) == 24, bands

    def spectra(class_name, acft):
        a, d = aircraft[acft].spectra if acft in aircraft else (
            str(int(float(anp.aircraft[acft]["Approach Spectral Class ID"]))),
            str(int(float(anp.aircraft[acft]["Departure Spectral Class ID"]))))
        spectrum = lambda cid: ", ".join(repr(float(anp.spectra[cid][band])) for band in bands)
        return (f'    Some(ClassSpectra {{ class_name: "{class_name}", anchor: "{acft}", '
                f"approach_class: {a}, departure_class: {d},\n"
                f"        approach_db: [{spectrum(a)}],\n        departure_db: [{spectrum(d)}] }}),")
    spectra_lines = [spectra("WING_FALLBACK", "737800")] + [spectra(a, a) for a in anp_ids] + ["    None,"]
    open(os.path.join(doc29, "spectra_generated.rs"), "w").write(
        """//! Generated by `scripts/aircraft-classes.py` from the EASA ANP database (v9 spectral classes and
//! aircraft, v2.3 aircraft): per noise class the unweighted 1/3-octave spectra (50 Hz - 10 kHz) of
//! its aircraft's approach and departure spectral classes at 305 m in the SAE AIR-1845 atmosphere
//! (Doc 29 4th ed. Vol 2 Appendix D); the fallback proxy takes the 737-800's. Not edited by hand.

use super::atmosphere::ClassSpectra;
use super::profiles_generated::NUM_CLASSES;

/// Per-noise-class spectra, in CLASS_NAMES order (none for the helicopter class).
pub static SPECTRA: [Option<ClassSpectra>; NUM_CLASSES] = [
""" + "\n".join(spectra_lines) + "\n];\n")
    for c, name in enumerate(class_names):
        print(f"{c:3d} {name:12s} {family[c]:10s} anchor {designators[anchors[c]]:5s} "
              f"members {sum(1 for k in class_of if k == c)}", file=sys.stderr)


if __name__ == "__main__":
    main(*sys.argv[1:4])
