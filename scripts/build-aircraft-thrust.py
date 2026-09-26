#!/usr/bin/env python3
"""Generate engine/noise-compute/src/emission/thrust_generated.rs.

Thrust-dependent NPD power rows (Doc 29 4th ed. Vol 2 Eq. 4-3/B-1/B-12) for the
jet class anchors, plus EASA-certified helicopter corrections. Stdlib only.

Inputs (all hashed into the banner; every per-anchor table is required):
  --anp DIR        EASA ANP v2.3 CSVs (NPD, jet coefficients, weights, aero, steps)
  --anp-v9-dir DIR EASA ANP v9 xlsx supplement (neo anchors; stdlib xlsx reader)
  --easa-xlsx PATH EASA Certification Noise Levels - Helicopters Iss. 52 workbook
  --counts PATH    pinned global traffic counts (GYRO prior, class-mean check)

Output (-o): Rust source for `thrust_generated.rs` (already rustfmt-clean).

Method (thrust): per jet anchor, all ANP NPD power rows (SEL + LAmax, paired by
power setting) for both op modes; B-1 coefficients (E,F,Ga,Gb,H) for the
MaxTakeoff/MaxClimb/IdleApproach ratings; engine count; the median DEFAULT
stage weight (observed stage lengths are unknown); the clean-configuration
drag ratio (minimum-R departure flap); the cutback height (end altitude of the
last MaxTakeoff step: DEFAULT stage 1, ICAO_A stage 1 for v9 anchors).
Pinned classes (fallback proxy, piston GA, turboprop % power, helicopters) emit
`ThrustModel::pinned` and keep today's max/min-row curves from PROFILES.

Method (helicopters): per typecode, Chapter 11 SEL energy mean over the
representative records, else Chapter 8 overflight EPNL energy mean minus D
(D = 2.65 dB, median over same-model/engine Ch11/Ch8 pairs); climb/descent
corrections from the median takeoff/approach-minus-overflight uplift
(DB-wide medians 1.3/4.1 dB where a typecode has no Ch8 pair). Corrections are
certified-minus-model at 150 m against the MV-22 shape curves. Typecode to EASA
model mapping after w5-aircraft build_certified_levels.py (not verified
against ICAO Doc 8643). GYRO (gyroplanes, absent from the DB) takes the
traffic-weighted light-helicopter mean as its prior.
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import math
import re
import statistics
import sys
import zipfile
import xml.etree.ElementTree as ET
from pathlib import Path

DIST_FT = [200, 400, 630, 1000, 2000, 4000, 6300, 10000, 16000, 25000]
NPD_COLS = [f"L_{d}ft" for d in DIST_FT]
MAX_POWER_ROWS = 6  # must equal thrust::MAX_POWER_ROWS

# Class order MUST equal CLASS_NAMES in profiles_generated.rs (pinned by test).
# (class, anchor typecode, anchor ANP ACFT_ID or None when pinned, anchor name)
ANCHORS: list[tuple[str, str, str | None, str]] = [
    ("WING_FALLBACK", "FALLBACK", None, "FALLBACK/737800"),
    ("WING_A320", "A320", "A320-232", "A320/A320-232"),
    ("WING_B738", "B738", "737800", "B738/737800"),
    ("PROP_C172", "C172", None, "C172/PISTON_SE"),
    ("WING_B38M", "B38M", "7378MAX", "B38M/7378MAX"),
    ("WING_B789", "B789", "7878R", "B789/7878R"),
    ("WING_A21N", "A21N", "A321-270N", "A21N/A321-270N"),
    ("WING_A321", "A321", "A321-232", "A321/A321-232"),
    ("WING_A20N", "A20N", "A320-270N", "A20N/A320-270N"),
    ("WING_A319", "A319", "A319-131", "A319/A319-131"),
    ("FUSE_CRJ9", "CRJ9", "CL601", "CRJ9/CL601"),
    ("WING_B748", "B748", "7478", "B748/7478"),
    ("HELICOPTER", "AS50", None, "AS50/HELICOPTER"),
    ("PROP_DH8D", "DH8D", None, "DH8D/DHC830"),
    ("FUSE_C56X", "C56X", "CIT3", "C56X/CIT3"),
]

# Helicopter typecode to profile index (profiles_generated.rs order 102..122).
HELI_PROFILE_IDX = {
    "EC35": 102, "EC45": 103, "EC55": 104, "EC30": 105, "EC20": 106,
    "AS50": 107, "AS55": 108, "AS65": 109, "H500": 110, "MD52": 111,
    "B06": 112, "B407": 113, "B412": 114, "R22": 115, "R44": 116,
    "R66": 117, "S76": 118, "A109": 119, "BK17": 120, "B505": 121,
    "GYRO": 122,
}

# Typecode to (all-records regex, representative regex); after w5-aircraft
# build_certified_levels.py. Mapping not verified against ICAO Doc 8643.
HELI_MAP = {
    "EC35": (r"^EC135 ", r"^EC135 (T2\+|P2\+|T3H|P3H|T1\(CDS\)|P1\(CDS\))$"),
    "EC45": (r"^MBB-BK117 (C-2|C-2e|D-2|D-2m|D-3|D-3m)$", r"^MBB-BK117 (C-2|D-2|D-3)$"),
    "EC55": (r"^EC 155 ", r"^EC 155 B1$"),
    "EC30": (r"^EC 130 ", r"^EC 130 (B4|T2)$"),
    "EC20": (r"^EC 120 ", r"^EC 120 B$"),
    "AS50": (r"^AS 350 ", r"^AS 350 (B2|B3)$"),
    "AS55": (r"^AS 355 ", r"^AS 355 (F2|N|NP)$"),
    "AS65": (r"^(AS 365|SA 365 N)", r"^AS 365 (N2|N3)$"),
    "H500": (r"^369(D|E|FF|HE|HS)$", r"^369(D|E)$"),
    "MD52": (r"^500N$", r"^500N$"),
    "B06": (r"^(206B|206L-4|206L-4T|AB206B)$", r"^(206B|206L-4)$"),
    "B407": (r"^407$", r"^407$"),
    "B412": (r"^(412|412EP|AB412|AB412EP)$", r"^(412|412EP)$"),
    "R22": (r"^R22", r"^R22 Beta$"),
    "R44": (r"^R44", r"^R44( II)?$"),
    "R66": (r"^R66$", r"^R66$"),
    "S76": (r"^S-76[A-D]$", r"^S-76(C|D)$"),
    "A109": (r"^(A109|AW109SP)", r"^(A109E|A109S|AW109SP)$"),
    "BK17": (r"^MBB-BK117 (A|B|C-1)", r"^MBB-BK117 (B-2|C-1)$"),
    "B505": (r"^505$", r"^505$"),
}

# Current MV-22-shape helicopter curves (dev1 MANUAL_PROFILES HELICOPTER);
# level/descent rows use approach, climb rows use departure. A Rust test pins
# these against the live PROFILES anchor, so a profile regen fails loudly.
HELI_MODEL_APPROACH = [99.3, 95.9, 93.5, 91.0, 86.6, 81.2, 77.4, 72.7, 66.7, 59.9]
HELI_MODEL_DEPARTURE = [97.3, 93.9, 91.5, 89.0, 84.6, 79.2, 75.4, 70.7, 64.7, 57.9]
M150_FT = 150.0 * 3.28084


def fail(msg: str) -> None:
    print(f"build-aircraft-thrust: error: {msg}", file=sys.stderr)
    sys.exit(1)


def sha_short(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()[:16]


# ─── stdlib xlsx reader (shared strings + inline strings; verified ───
# ─── byte-identical to openpyxl on the ANP v9 NPD sheet, 544 rows) ────
NS = {"m": "http://schemas.openxmlformats.org/spreadsheetml/2006/main"}


def xlsx_sheet_rows(path: Path, want: str) -> list[list[str]]:
    z = zipfile.ZipFile(path)
    wb = ET.fromstring(z.read("xl/workbook.xml"))
    sheets = [(s.get("name"), s.get("{http://schemas.openxmlformats.org/officeDocument/2006/relationships}id"))
              for s in wb.iter(f"{{{NS['m']}}}sheet")]
    rels = ET.fromstring(z.read("xl/_rels/workbook.xml.rels"))
    target = {r.get("Id"): r.get("Target") for r in rels.iter(
        "{http://schemas.openxmlformats.org/package/2006/relationships}Relationship")}
    files = {name: target[rid].replace("worksheets/", "") for name, rid in sheets}
    if want not in files:
        fail(f"{path.name}: sheet {want!r} missing (have {sorted(files)})")
    try:
        sst = ET.fromstring(z.read("xl/sharedStrings.xml"))
        strings = ["".join(t.text or "" for t in si.iter(f"{{{NS['m']}}}t"))
                   for si in sst.findall("m:si", NS)]
    except KeyError:
        strings = []

    def col(ref: str) -> int:
        n = 0
        for ch in re.match(r"[A-Z]+", ref).group():
            n = n * 26 + ord(ch) - 64
        return n - 1

    root = ET.fromstring(z.read(f"xl/worksheets/{files[want]}"))
    by_row: dict[int, list[str]] = {}
    top = 0
    for r in root.iter(f"{{{NS['m']}}}row"):
        vals: dict[int, str] = {}
        for c in r.findall("m:c", NS):
            v = c.find("m:v", NS)
            t = c.get("t")
            if t == "inlineStr":
                val = "".join(x.text or "" for x in c.iter(f"{{{NS['m']}}}t"))
            elif v is None:
                continue
            elif t == "s":
                val = strings[int(v.text)]
            elif t == "e":
                continue
            else:
                val = v.text
            vals[col(c.get("r"))] = val
        if vals:
            n = int(r.get("r"))
            by_row[n] = [vals.get(i, "") for i in range(max(vals) + 1)]
            top = max(top, n)
    # Keep sparse row positions: a skipped blank header row must not shift indices.
    return [by_row.get(n, []) for n in range(1, top + 1)]


def xlsx_dicts(path: Path, sheet: str, key_col: str) -> list[dict[str, str]]:
    rows = xlsx_sheet_rows(path, sheet)
    header = rows[0]
    if key_col not in header:
        fail(f"{path.name}/{sheet}: column {key_col!r} missing")
    return [dict(zip(header, r)) for r in rows[1:] if r and r[0] != ""]


# ─── ANP loaders ──────────────────────────────────────────────────────
def load_csv(path: Path) -> list[dict[str, str]]:
    with path.open() as fh:
        return list(csv.DictReader(fh, delimiter=";"))


class Anp:
    def __init__(self, anp_dir: Path, v9_dir: Path):
        req = ["ANP2.3_Aircraft.csv", "ANP2.3_NPD_data.csv",
               "ANP2.3_Jet_engine_coefficients.csv", "ANP2.3_Default_weights.csv",
               "ANP2.3_Aerodynamic_coefficients.csv",
               "ANP2.3_Default_departure_procedural_steps.csv"]
        for name in req:
            if not (anp_dir / name).exists():
                fail(f"ANP v2.3 file missing: {name}")
        self.aircraft = {r["ACFT_ID"]: r for r in load_csv(anp_dir / "ANP2.3_Aircraft.csv")}
        self.npd: dict[tuple[str, str, str], list[tuple[float, list[float]]]] = {}
        for r in load_csv(anp_dir / "ANP2.3_NPD_data.csv"):
            self.npd.setdefault((r["NPD_ID"], r["Noise Metric"], r["Op Mode"]), []).append(
                (float(r["Power Setting"]), [float(r[c]) for c in NPD_COLS]))
        self.jet = {(r["ACFT_ID"], r["Thrust Rating"]): r
                    for r in load_csv(anp_dir / "ANP2.3_Jet_engine_coefficients.csv")}
        self.weights: dict[tuple[str, str], float] = {}
        for r in load_csv(anp_dir / "ANP2.3_Default_weights.csv"):
            self.weights[(r["ACFT_ID"], r["Stage Length"])] = float(r["Weight (lb)"])
        self.aero = {(r["ACFT_ID"], r["Op Type"], r["Flap_ID"].strip()): r
                     for r in load_csv(anp_dir / "ANP2.3_Aerodynamic_coefficients.csv")}
        self.steps = load_csv(anp_dir / "ANP2.3_Default_departure_procedural_steps.csv")
        # v9 supplement wins on collision (same rule as build-aircraft-profiles.py).
        v9files = {
            "EASA_ANP_database_Aircraft_v9.xlsx": ("ACFT_ID", None),
            "EASA_ANP_database_NPD_Data_v9.xlsx": ("NPD_ID", None),
            "EASA_ANP_database_Jet_Engine_Coefficients_v9.xlsx": ("ACFT_ID", None),
            "EASA_ANP_database_Default_Weights_v9.xlsx": ("ACFT_ID", None),
            "EASA_ANP_database_Aerodynamic_Coefficients_v9.xlsx": ("ACFT_ID", None),
            "EASA_ANP_database_Default_Departure_Procedural_Steps_v9.xlsx": ("ACFT_ID", None),
        }
        for name in v9files:
            if not (v9_dir / name).exists():
                fail(f"ANP v9 file missing: {name}")
        def first_sheet(name: str) -> str:
            zf = zipfile.ZipFile(v9_dir / name)
            wb = ET.fromstring(zf.read("xl/workbook.xml"))
            return [s.get("name") for s in wb.iter(f"{{{NS['m']}}}sheet")][0]

        v9ac = "EASA_ANP_database_Aircraft_v9.xlsx"
        for r in xlsx_dicts(v9_dir / v9ac, first_sheet(v9ac), "ACFT_ID"):
            self.aircraft[str(r["ACFT_ID"])] = {k: str(v) for k, v in r.items()}
        v9npd = "EASA_ANP_database_NPD_Data_v9.xlsx"
        v9_npd: dict[tuple[str, str, str], list[tuple[float, list[float]]]] = {}
        for r in xlsx_dicts(v9_dir / v9npd, first_sheet(v9npd), "NPD_ID"):
            try:
                v9_npd.setdefault((str(r["NPD_ID"]), str(r["Noise Metric"]),
                                   str(r["Op Mode"])), []).append(
                    (float(r["Power Setting"]), [float(r[c]) for c in NPD_COLS]))
            except (TypeError, ValueError):
                continue
        for k in v9_npd:
            v9_npd[k].sort()
        self.npd.update(v9_npd)  # v9 wins on collision, like build-aircraft-profiles.py
        for k in self.npd:
            self.npd[k].sort()
        v9jet = "EASA_ANP_database_Jet_Engine_Coefficients_v9.xlsx"
        for r in xlsx_dicts(v9_dir / v9jet, first_sheet(v9jet), "ACFT_ID"):
            self.jet[(str(r["ACFT_ID"]), str(r["Thrust Rating"]))] = r
        v9w = "EASA_ANP_database_Default_Weights_v9.xlsx"
        for r in xlsx_dicts(v9_dir / v9w, first_sheet(v9w), "ACFT_ID"):
            try:
                self.weights[(str(r["ACFT_ID"]), str(r["Stage Length"]))] = \
                    float(r["Weight (lb)"])
            except (TypeError, ValueError):
                continue
        v9ae = "EASA_ANP_database_Aerodynamic_Coefficients_v9.xlsx"
        for r in xlsx_dicts(v9_dir / v9ae, first_sheet(v9ae), "ACFT_ID"):
            self.aero[(str(r["ACFT_ID"]), str(r["Op Type"]), str(r["Flap_ID"]).strip())] = \
                {k: (str(v) if v is not None else "") for k, v in r.items()}
        v9st = "EASA_ANP_database_Default_Departure_Procedural_Steps_v9.xlsx"
        self.steps += [{k: (str(v) if v is not None else "")
                        for k, v in r.items()} for r in xlsx_dicts(
            v9_dir / v9st, first_sheet(v9st), "ACFT_ID")]


def energy_mean(xs: list[float]) -> float:
    return 10.0 * math.log10(sum(10.0 ** (x / 10.0) for x in xs) / len(xs))


def at_dist_loglinear(curve: list[float], d_ft: float) -> float:
    ld = math.log10(d_ft)
    logs = [math.log10(d) for d in DIST_FT]
    for i in range(9):
        if d_ft <= DIST_FT[i + 1]:
            f = (ld - logs[i]) / (logs[i + 1] - logs[i])
            return curve[i] + f * (curve[i + 1] - curve[i])
    raise ValueError(d_ft)


# ─── thrust extraction ───────────────────────────────────────────────
def fn_delta_b1(coef: dict[str, str], vc_kt: float, h_ft: float, t_c: float) -> float:
    """Doc 29 Vol 2 Eq. B-1 corrected net thrust per engine (lb)."""
    g = lambda k: float(coef.get(k) or 0)  # noqa: E731
    return g("E") + g("F") * vc_kt + g("Ga") * h_ft + g("Gb") * h_ft * h_ft + g("H") * t_c


def npd_eq43(rows: list[tuple[float, list[float]]], p: float) -> tuple[list[float], int, float]:
    """Doc 29 Eq. 4-3 linear-in-power interpolation; clamps outside the table."""
    ps = [r[0] for r in rows]
    if p <= ps[0]:
        return rows[0][1], 0, 0.0
    if p >= ps[-1]:
        return rows[-1][1], len(ps) - 1, 0.0
    i = max(k for k in range(len(ps) - 1) if ps[k] <= p)
    (p0, l0), (p1, l1) = rows[i], rows[i + 1]
    w = (p - p0) / (p1 - p0)
    return [a + (b - a) * w for a, b in zip(l0, l1)], i, w


class ThrustRow:
    def __init__(self, anp: Anp, acft_id: str):
        if acft_id not in anp.aircraft:
            fail(f"ACFT_ID {acft_id} missing from ANP Aircraft table")
        acft = anp.aircraft[acft_id]
        if acft.get("Engine Type") != "Jet":
            fail(f"{acft_id}: Engine Type {acft.get('Engine Type')!r} is not Jet")
        if "lb" not in (acft.get("Power Parameter") or ""):
            fail(f"{acft_id}: Power Parameter {acft.get('Power Parameter')!r} is not lb")
        self.engines = int(float(acft["Number Of Engines"]))
        if self.engines < 1:
            fail(f"{acft_id}: bad engine count")
        npd_id = acft["NPD_ID"]
        self.op: dict[str, dict[str, list]] = {}
        for mode in ("A", "D"):
            sel = anp.npd.get((npd_id, "SEL", mode), [])
            lmax = anp.npd.get((npd_id, "LAmax", mode), [])
            if len(sel) < 2 or [p for p, _ in sel] != [p for p, _ in lmax]:
                fail(f"{acft_id}/{npd_id} op {mode}: SEL/LAmax power rows differ "
                     f"({[p for p, _ in sel]} vs {[p for p, _ in lmax]})")
            if len(sel) > MAX_POWER_ROWS:
                fail(f"{acft_id}/{npd_id} op {mode}: {len(sel)} rows exceed {MAX_POWER_ROWS}")
            self.op[mode] = {"power": [p for p, _ in sel],
                             "sel": [v for _, v in sel],
                             "lmax": [v for _, v in lmax]}
        self.coef: dict[str, list[float]] = {}
        for rating in ("MaxTakeoff", "MaxClimb", "IdleApproach"):
            c = anp.jet.get((acft_id, rating))
            if c is None:
                if rating != "IdleApproach":
                    fail(f"{acft_id}: thrust rating {rating} missing")
                # Older ANP entries (CL601, CIT3) tabulate no idle rating; the
                # Eq. 4-3 clamp to the lowest power row is the floor instead.
                print(f"warning: {acft_id} has no IdleApproach rating; "
                      f"idle floor falls back to the lowest tabulated row",
                      file=sys.stderr)
                self.coef[rating] = [0.0] * 5
                continue
            vals = [float(c.get(k) or 0) for k in ("E", "F", "Ga", "Gb", "H")]
            if vals[4] != 0.0:
                fail(f"{acft_id}/{rating}: H={vals[4]} needs ambient temperature data")
            self.coef[rating] = vals
        stage_w = sorted(w for (a, s), w in anp.weights.items()
                         if a == acft_id and s.isdigit())
        if not stage_w:
            fail(f"{acft_id}: no numeric DEFAULT stage weights")
        self.weight_lb = statistics.median(stage_w)
        d_flaps = [(float(r["R"]), fid) for (a, op, fid), r in anp.aero.items()
                   if a == acft_id and op == "D" and (r.get("R") or "") != ""]
        if not d_flaps:
            fail(f"{acft_id}: no departure aerodynamic coefficients")
        self.drag_ratio, self.drag_flap = min(d_flaps)
        if not 0.0 < self.drag_ratio < 0.3:
            fail(f"{acft_id}: implausible clean R {self.drag_ratio}")
        steps = [r for r in anp.steps if r["ACFT_ID"] == acft_id
                 and r["Stage Length"] == "1"
                 and r["Profile_ID"] in ("DEFAULT", "ICAO_A")]
        if any(r["Profile_ID"] == "DEFAULT" for r in steps):
            steps = [r for r in steps if r["Profile_ID"] == "DEFAULT"]
        steps.sort(key=lambda r: int(float(r["Step Number"])))
        self.cutback_ft = None
        for i, r in enumerate(steps):
            if r["Thrust Rating"] == "MaxClimb":
                prev = [s for s in steps[:i] if s["Thrust Rating"] == "MaxTakeoff"
                        and (s.get("End Point Altitude (ft)") or "") != ""]
                if prev:
                    self.cutback_ft = float(prev[-1]["End Point Altitude (ft)"])
                break
        if self.cutback_ft is None or not 500.0 <= self.cutback_ft <= 5000.0:
            fail(f"{acft_id}: no usable cutback height (got {self.cutback_ft})")
        self.coef_rows = {}
        for rating in ("MaxTakeoff", "MaxClimb", "IdleApproach"):
            if (acft_id, rating) in anp.jet:
                self.coef_rows[rating] = anp.jet[(acft_id, rating)]


# ─── EASA helicopter extraction ──────────────────────────────────────
def parse_easa(path: Path) -> list[dict]:
    rows = xlsx_sheet_rows(path, "HELICOPTERS")
    if rows[2][7] != "TYPE DESIGNATION / MODEL" or rows[2][11] != "MTOM" \
            or rows[2][15] != "TYPE DESIGNATION / MODEL" \
            or rows[1][23] != "CHAPTER 11 NOISE LEVELS (dBA SEL)" \
            or rows[1][27] != "CHAPTER 8 NOISE LEVELS (EPNdB)":
        fail("EASA workbook header layout changed; column map needs review")

    def num(v) -> float | None:
        if v in ("", None):
            return None
        try:
            return round(float(v), 1)
        except ValueError:
            return None

    out = []
    for r in rows[5:]:
        r = r + [""] * 45
        model = (r[7] or "").strip()
        if not model or model.startswith("("):
            continue
        out.append({"model": model, "mtom_kg": num(r[11]), "engine": (r[15] or "").strip(),
                    "ch11_sel": num(r[23]),
                    "ch8_takeoff": num(r[27]), "ch8_overflight": num(r[30]),
                    "ch8_approach": num(r[33])})
    # 496 model-bearing records (the evidence's "502" counts record-number
    # stubs; this filter is multiset-identical to w5-aircraft's sheet1.csv).
    if len(out) != 496:
        fail(f"EASA workbook has {len(out)} model records, expected 496 (Issue 52)")
    return out


PISTON_ENGINE = re.compile(r"(IO|TIO|HIO|GO|GIO|TIGO|VO|O)-?\d{2,3}\b")


class HeliLevels:
    def __init__(self, records: list[dict], counts: dict[str, int]):
        # D: median of per-engine medians over Ch11/Ch8-overflight pairs. Each
        # Ch11 record pairs with the closest-MTOM Ch8-overflight record of the
        # same model and engine; grouping by engine folds CDS/CPDS avionics
        # variants of one rotorcraft. Piston pairs are out of scope (every
        # typecode converted with D is turbine-powered).
        ch8_by_model_engine: dict[tuple[str, str], list[dict]] = {}
        for x in records:
            if x["ch8_overflight"] is not None and x["mtom_kg"] is not None:
                ch8_by_model_engine.setdefault((x["model"], x["engine"]), []).append(x)
        by_engine: dict[str, list[float]] = {}
        for x in records:
            if x["ch11_sel"] is None or x["mtom_kg"] is None:
                continue
            if PISTON_ENGINE.search(x["engine"] or ""):
                continue
            cands = ch8_by_model_engine.get((x["model"], x["engine"]), [])
            if not cands:
                continue
            best = min(cands, key=lambda c: abs(c["mtom_kg"] - x["mtom_kg"]))
            by_engine.setdefault(x["engine"], []).append(best["ch8_overflight"] - x["ch11_sel"])
        if not by_engine:
            fail("EASA workbook: no Ch11/Ch8 same-model/engine pairs for D")
        self.epnl_minus_sel = statistics.median(
            statistics.median(v) for v in by_engine.values())
        self.d_groups = {k: round(statistics.median(v), 2) for k, v in by_engine.items()}
        db_ap = [x["ch8_approach"] - x["ch8_overflight"] for x in records
                 if x["ch8_approach"] is not None and x["ch8_overflight"] is not None]
        db_to = [x["ch8_takeoff"] - x["ch8_overflight"] for x in records
                 if x["ch8_takeoff"] is not None and x["ch8_overflight"] is not None]
        self.db_approach_uplift = statistics.median(db_ap)
        self.db_takeoff_uplift = statistics.median(db_to)
        self.per_typecode: dict[str, dict] = {}
        for tc, (pat_all, pat_rep) in HELI_MAP.items():
            allr = [x for x in records if re.search(pat_all, x["model"])]
            reps = [x for x in allr
                    if re.search(pat_rep, x["model"])
                    and (x["ch11_sel"] is not None or x["ch8_overflight"] is not None)]
            if not reps:
                fail(f"helicopter {tc}: no representative EASA records")
            ch11s = [x["ch11_sel"] for x in reps if x["ch11_sel"] is not None]
            if ch11s:
                sel150, basis, n_sel = energy_mean(ch11s), "ch11", len(ch11s)
            else:
                ofs = [x["ch8_overflight"] for x in reps if x["ch8_overflight"] is not None]
                sel150, basis, n_sel = \
                    energy_mean(ofs) - self.epnl_minus_sel, "ch8-d", len(ofs)
            pairs = [(x["ch8_overflight"], x["ch8_approach"], x["ch8_takeoff"])
                     for x in reps
                     if x["ch8_overflight"] is not None and x["ch8_approach"] is not None]
            ap = statistics.median(b - a for a, b, _ in pairs) if pairs \
                else self.db_approach_uplift
            tos = [c - a for a, _, c in pairs if c is not None]
            to = statistics.median(tos) if tos else self.db_takeoff_uplift
            # MTOM class from the representative records: the DB's 369HE/HS
            # rows carry a 6000 kg MTOM quirk (the airframe is a 1.4 t 500).
            mtom = max((x["mtom_kg"] for x in reps if x["mtom_kg"] is not None), default=None)
            self.per_typecode[tc] = {"sel150": sel150, "basis": basis,
                                     "approach_uplift": ap, "takeoff_uplift": to,
                                     "n_sel": n_sel, "mtom_kg": mtom,
                                     "traffic": counts.get(tc, 0)}
        # Weight classes (Ch11 3200 kg / Ch8 5000 kg breaks) and their
        # traffic-weighted energy-mean level SEL at 150 m.
        self.class_mean: dict[str, float] = {}
        for cls, lo, hi in (("light", 0, 3200), ("medium", 3200, 5000), ("heavy", 5000, 10**9)):
            members = [v for v in self.per_typecode.values()
                       if v["mtom_kg"] is not None and lo < v["mtom_kg"] <= hi
                       and v["traffic"] > 0]
            if not members:
                fail(f"helicopter class {cls}: no members with traffic")
            e = sum(v["traffic"] * 10.0 ** (v["sel150"] / 10.0) for v in members)
            self.class_mean[cls] = 10.0 * math.log10(e / sum(v["traffic"] for v in members))
        light = [v for v in self.per_typecode.values()
                 if v["mtom_kg"] is not None and v["mtom_kg"] <= 3200 and v["traffic"] > 0]
        tot = sum(v["traffic"] for v in light)
        self.light_mean_corr = {
            k: sum(v["traffic"] * v[k] for v in light) / tot
            for k in ("sel150", "approach_uplift", "takeoff_uplift")}


# ─── emitter ─────────────────────────────────────────────────────────
def f(x: float) -> str:
    s = repr(float(x))
    return s if "." in s or "e" in s else s + ".0"


def rust_array_lines(values: list[str], indent: int) -> list[str]:
    """One-line array, or rustfmt's wrapped form when the body tops 60 chars."""
    body = ", ".join(values)
    pad = " " * indent
    if len(body) <= 60:
        return [f"{pad}[{body}],"]
    return [f"{pad}[", f"{pad}    {body},", f"{pad}],"]


def emit(thrust: dict[str, ThrustRow], heli: HeliLevels, hashes: dict[str, str],
         counts_hash: str) -> str:
    model_level = at_dist_loglinear(HELI_MODEL_APPROACH, M150_FT)
    model_climb = at_dist_loglinear(HELI_MODEL_DEPARTURE, M150_FT)
    lines = [
        "//! Auto-generated by `scripts/build-aircraft-thrust.py`. DO NOT EDIT BY HAND.",
        "//!",
        "//! Per-class thrust power rows (Doc 29 Eq. 4-3/B-1/B-12) and EASA-certified",
        "//! helicopter corrections. Regen: `python3 scripts/build-aircraft-thrust.py \\",
        "//!   --anp <ANP2.3-DIR> --anp-v9-dir <V9-DIR> --easa-xlsx <XLSX>",
        "//!   --counts scripts/aircraft-profiles-counts.json -o \\",
        "//!   engine/noise-compute/src/emission/thrust_generated.rs`.",
        "//!",
        "//! Inputs:",
    ]
    for name in sorted(hashes):
        lines.append(f"//!   {name}: sha256={hashes[name]}")
    lines += [
        f"//!   counts: sha256={counts_hash}",
        "//!",
        "//! Helicopter data: EASA Certification Noise Levels - Helicopters, Issue 52",
        "//! (26 Jun 2026), https://www.easa.europa.eu/en/downloads/16971/en .",
        "//! Reproduction authorised provided the source is acknowledged.",
        "",
        "use super::aircraft::{HeliCorrection, ThrustModel};",
        "use super::profiles_generated::{NUM_CLASSES, NUM_PROFILES};",
        "",
        "/// Per-noise-class thrust model, in CLASS_NAMES order (pinned by test).",
        "pub static THRUST: [ThrustModel; NUM_CLASSES] = [",
    ]
    for cls, tc, acft_id, anchor in ANCHORS:
        if acft_id is None:
            lines.append(f'    ThrustModel::pinned("{cls}", "{anchor}"),')
            continue
        t = thrust[tc]
        lines.append(f"    // {cls} <- {acft_id} (median stage weight, R from clean flap {t.drag_flap})")
        lines.append("    ThrustModel {")
        lines.append(f'        class_name: "{cls}",')
        lines.append(f'        anchor_name: "{anchor}",')
        lines.append("        has_thrust: true,")
        lines.append(f"        engines: {t.engines},")
        lines.append(f"        weight_lb: {f(t.weight_lb)},")
        lines.append(f"        drag_ratio: {f(t.drag_ratio)},")
        lines.append(f"        cutback_ft_afe: {f(t.cutback_ft)},")
        lines.append("        // B-1 coefficients (E, F, Ga, Gb, H) per rating.")
        for rating, field in (("MaxTakeoff", "takeoff_coef"), ("MaxClimb", "climb_coef"),
                              ("IdleApproach", "idle_coef")):
            c = t.coef[rating]
            lines.append(f"        {field}: [{', '.join(f(v) for v in c)}],")
        for mode, field in (("D", "dep"), ("A", "app")):
            o = t.op[mode]
            n = len(o["power"])
            pad = lambda vs, last: vs + [last] * (MAX_POWER_ROWS - n)  # noqa: E731
            lines.append(f"        {field}_rows: {n},")
            lines.append(f"        {field}_power: [{', '.join(f(v) for v in pad(o['power'], o['power'][-1]))}],")
            for metric in ("sel", "lmax"):
                lines.append(f"        {field}_{metric}: [")
                # 1-decimal NPD levels: the profiles_generated.rs convention
                # (v9's second decimal is 0.005 dB).
                for row in pad(o[metric], o[metric][-1]):
                    lines.extend(rust_array_lines([f(round(v, 1)) for v in row], 12))
                lines.append("        ],")
        lines.append("    },")
    lines += [
        "];",
        "",
        "/// Per-profile helicopter certification correction (dB, added to the looked-up",
        "/// NPD SEL/LAmax): certified level/climb/descent SEL at 150 m minus the model",
        f"/// curve actually used ({model_level:.2f} approach / {model_climb:.2f} departure dB).",
        "/// Non-helicopter profiles are zero. GYRO takes the light-class prior.",
        "pub static HELI_CORRECTIONS: [HeliCorrection; NUM_PROFILES] = [",
    ]
    per_tc_corr = {}
    for tc, v in heli.per_typecode.items():
        per_tc_corr[tc] = {
            "level": v["sel150"] - model_level,
            "climb": v["sel150"] + v["takeoff_uplift"] - model_climb,
            "descent": v["sel150"] + v["approach_uplift"] - model_level,
        }
    lm = heli.light_mean_corr
    per_tc_corr["GYRO"] = {
        "level": lm["sel150"] - model_level,
        "climb": lm["sel150"] + lm["takeoff_uplift"] - model_climb,
        "descent": lm["sel150"] + lm["approach_uplift"] - model_level,
    }
    idx_of = {v: k for k, v in HELI_PROFILE_IDX.items()}
    for idx in range(124):
        tc = idx_of.get(idx)
        if tc is None:
            lines.append("    HeliCorrection::zero(),")
            continue
        c = per_tc_corr[tc]
        v = heli.per_typecode.get(tc)
        note = (f"sel150 {v['sel150']:.1f} ({v['basis']}, n {v['n_sel']}), "
                f"climb +{v['takeoff_uplift']:.1f}, descent +{v['approach_uplift']:.1f}"
                if v is not None else "light-class traffic-weighted prior")
        lines.append(f"    // {tc}: {note}")
        lines.append("    HeliCorrection {")
        lines.append(f"        level_db: {f(c['level'])},")
        lines.append(f"        climb_db: {f(c['climb'])},")
        lines.append(f"        descent_db: {f(c['descent'])},")
        lines.append("    },")
    lines.append("];")
    return "\n".join(lines) + "\n"


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--anp", required=True, type=Path)
    ap.add_argument("--anp-v9-dir", required=True, type=Path)
    ap.add_argument("--easa-xlsx", required=True, type=Path)
    ap.add_argument("--counts", required=True, type=Path)
    ap.add_argument("-o", required=True, type=Path)
    args = ap.parse_args()

    anp = Anp(args.anp, args.anp_v9_dir)
    thrust = {}
    for cls, tc, acft_id, _ in ANCHORS:
        if acft_id is not None:
            thrust[tc] = ThrustRow(anp, acft_id)

    # Golden: B738 at 3,000 ft AFE after cutback (ANP DEFAULT stage 1 step 6).
    b = thrust["B738"]
    p_sched = fn_delta_b1(b.coef_rows["MaxClimb"], 204.8, 3000.0 + 355.5 / 0.3048, 15.0)
    if abs(p_sched - 18093.0) > 0.5:
        fail(f"B738 B-1 golden: Fn/delta {p_sched:.1f} != 18093.0")
    rows_d = list(zip(b.op["D"]["power"], b.op["D"]["sel"]))
    curve, row, w = npd_eq43([(p, v) for p, v in rows_d], p_sched)
    if abs(at_dist_loglinear(curve, 1000) - 93.77) > 0.01:
        fail("B738 Eq. 4-3 golden: SEL@1000ft != 93.77")
    if abs(at_dist_loglinear(curve, 3000) - 86.08) > 0.02:
        fail("B738 Eq. 4-3 golden: SEL@3000ft != 86.08")

    counts = json.loads(args.counts.read_text())
    heli = HeliLevels(parse_easa(args.easa_xlsx), counts)
    if abs(heli.epnl_minus_sel - 2.65) > 0.005:
        fail(f"EASA D golden: {heli.epnl_minus_sel:.3f} != 2.65")
    for cls, want in (("light", 83.1), ("medium", 84.4), ("heavy", 89.7)):
        if abs(heli.class_mean[cls] - want) > 0.05:
            fail(f"helicopter class {cls}: {heli.class_mean[cls]:.2f} != {want}")

    hashes = {}
    for p in sorted(args.anp.glob("ANP2.3_*.csv")):
        hashes[p.name] = sha_short(p)
    for p in sorted(args.anp_v9_dir.glob("EASA_ANP_database_*.xlsx")):
        hashes[p.name] = sha_short(p)
    hashes[args.easa_xlsx.name] = sha_short(args.easa_xlsx)
    args.o.write_text(emit(thrust, heli, hashes, sha_short(args.counts)))

    print(f"{'anchor':6} {'eng':3} {'cutback_ft':10} {'W_lb':>8} {'R':>8} "
          f"{'dep_rows':>8} {'app_rows':>8}")
    for _, tc, acft_id, _ in ANCHORS:
        if acft_id is None:
            print(f"{tc:6} pinned")
            continue
        t = thrust[tc]
        print(f"{tc:6} {t.engines:3} {t.cutback_ft:10.0f} {t.weight_lb:8.0f} "
              f"{t.drag_ratio:8.5f} {t.op['D']['power']} {t.op['A']['power']}")
    print(f"EASA D={heli.epnl_minus_sel:.2f} class means:",
          " ".join(f"{k}={v:.1f}" for k, v in heli.class_mean.items()))


if __name__ == "__main__":
    main()

